package sh.cosmos.companion.net

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.longOrNull

/**
 * How the companion authenticates and which edge it talks to. Dev mode mirrors
 * apps/ios AppConfig: the bearer is literally `userId@orgId` against an
 * AUTH_MODE=dev edge. Woozlit/WorkOS token modes reuse the same room URLs with
 * a real bearer (a follow-up; documented in the app).
 */
@Serializable
data class Connection(
    val edgeUrl: String,
    val userId: String,
    val orgId: String,
    val token: String,
    val deviceId: String,
    val deviceName: String,
    val mode: String = "dev",
) {
    /** Strip a trailing slash so path joins are clean. */
    val edgeBase: String get() = edgeUrl.trimEnd('/')
}

/** Row kinds — the four registry tables (crates/doc/src/registry.rs). */
object RowKind {
    const val DEVICES = "devices"
    const val SPACES = "spaces"
    const val CHATS = "chats"
    const val SESSIONS = "sessions"
}

/** One registry row exactly as the edge stores/returns it. */
@Serializable
data class RegistryRow(
    val kind: String,
    val id: String,
    val seq: Long = 0,
    val deleted: Boolean = false,
    val delHlc: String? = null,
    val fields: JsonObject = JsonObject(emptyMap()),
    val clocks: JsonObject = JsonObject(emptyMap()),
)

/** GET /registry/{org}/rows response. */
@Serializable
data class RowsResponse(
    val seq: Long = 0,
    val full: Boolean = true,
    val gcFloor: Long = 0,
    val rows: List<RegistryRow> = emptyList(),
    val presence: Map<String, Long> = emptyMap(),
)

/** One op in a push batch (crates/doc RowOp). */
@Serializable
data class RowOp(
    val kind: String,
    val id: String,
    val op: String,
    val set: Map<String, JsonElement>? = null,
    val hlc: String,
    val clocks: Map<String, String>? = null,
)

@Serializable
data class PushBatch(val batch: String, val ops: List<RowOp>)

@Serializable
data class PushAck(val batch: String? = null, val seq: Long = 0, val applied: Int = 0)

enum class SessionStatus { idle, working, awaitingInput, errored, unknown }

/** entities.rs indicator ranks — drives the status dot. */
enum class ChatIndicator { awaitingInput, errored, working, completed, idle }

/** A session as the companion displays it — chat row joined with its live
 *  session-status row. All fields come straight from the registry. */
data class SessionSummary(
    val chatId: String,
    val title: String,
    val harness: String?,
    val model: String?,
    val reasoning: String?,
    val branch: String?,
    val cwd: String?,
    val spaceId: String?,
    val deviceId: String?,
    val archived: Boolean,
    val lastMessagePreview: String?,
    val lastMessageAt: Long?,
    val createdAt: Long,
    val lastSeenAt: Long?,
    val status: SessionStatus,
    val statusUpdatedAt: Long?,
) {
    val unseen: Boolean
        get() {
            val at = lastMessageAt ?: return false
            val seen = lastSeenAt ?: return true
            return at > seen
        }

    /** state.rs staleness: a Working/AwaitingInput row older than 45s is stale. */
    fun indicator(now: Long): ChatIndicator {
        val stale = statusUpdatedAt != null && now - statusUpdatedAt > 45_000
        val live = if (stale && (status == SessionStatus.working || status == SessionStatus.awaitingInput))
            SessionStatus.unknown else status
        return when (live) {
            SessionStatus.working -> ChatIndicator.working
            SessionStatus.awaitingInput -> ChatIndicator.awaitingInput
            SessionStatus.errored -> if (unseen) ChatIndicator.errored else ChatIndicator.idle
            else -> if (unseen) ChatIndicator.completed else ChatIndicator.idle
        }
    }
}

data class SpaceInfo(val id: String, val path: String, val name: String?, val deviceId: String) {
    val displayName: String get() = name?.takeIf { it.isNotEmpty() } ?: path.substringAfterLast('/')
}

data class DeviceInfo(
    val id: String,
    val name: String,
    val platform: String,
    val lastSeenAt: Long?,
) {
    fun online(now: Long, presence: Map<String, Long>): Boolean {
        val beat = presence[id] ?: lastSeenAt ?: return false
        return now - beat < 45_000
    }
}

// ── field extraction helpers (fields is an untyped JSON bag) ──────────────────

internal fun JsonObject.str(key: String): String? =
    this[key]?.jsonPrimitive?.contentOrNull

internal fun JsonObject.long(key: String): Long? =
    this[key]?.jsonPrimitive?.longOrNull

internal fun JsonObject.bool(key: String): Boolean =
    this[key]?.jsonPrimitive?.booleanOrNull ?: false

internal fun JsonObject.obj(key: String): JsonObject? = this[key] as? JsonObject
