package sh.cosmos.companion.net

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

/**
 * The companion's edge client. Speaks the exact registry row-table protocol
 * the desktop and iOS clients use (edge/src/registry-room.ts,
 * crates/doc/src/registry.rs): pull deltas over `GET /registry/{org}/rows`,
 * push LWW ops over `POST /registry/{org}/push`. No Loro needed — the registry
 * is plain JSON rows with per-field HLC clocks.
 */
class CosmosClient(private val connection: Connection) {

    private val json = Json { ignoreUnknownKeys = true; encodeDefaults = true }
    private val http = OkHttpClient.Builder()
        .connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(30, TimeUnit.SECONDS)
        .build()

    // Local replica: kind -> id -> row, plus the sync cursor.
    private val rows = HashMap<String, HashMap<String, RegistryRow>>()
    private var cursor: Long = 0
    private var presence: Map<String, Long> = emptyMap()

    // Monotonic HLC clock for local writes: {ms:013}-{counter:06}-{device}.
    private var lastMs = 0L
    private var counter = 0L
    private val seqGen = AtomicLong(0)

    val conn: Connection get() = connection

    private fun url(path: String): String = "${connection.edgeBase}$path"

    private fun authed(builder: Request.Builder): Request.Builder =
        builder.header("Authorization", "Bearer ${connection.token}")

    /** GET /health — confirms the edge is reachable and reports auth mode. */
    suspend fun health(): String = withContext(Dispatchers.IO) {
        http.newCall(Request.Builder().url(url("/health")).build()).execute().use { r ->
            if (!r.isSuccessful) throw CosmosException("health ${r.code}")
            r.body?.string() ?: ""
        }
    }

    /** Pull the registry delta and merge it into the local replica. */
    suspend fun pull(): Unit = withContext(Dispatchers.IO) {
        val sinceParam = if (cursor > 0) "&since=$cursor" else ""
        val u = url("/registry/${connection.orgId}/rows?device=${connection.deviceId}&beat=1$sinceParam")
        http.newCall(authed(Request.Builder().url(u)).build()).execute().use { r ->
            val body = r.body?.string().orEmpty()
            if (!r.isSuccessful) throw CosmosException("rows ${r.code}: $body")
            val resp = json.decodeFromString(RowsResponse.serializer(), body)
            if (resp.full) rows.clear()
            for (row in resp.rows) {
                val table = rows.getOrPut(row.kind) { HashMap() }
                if (row.deleted) table.remove(row.id) else table[row.id] = row
            }
            cursor = resp.seq
            presence = resp.presence
        }
    }

    fun currentPresence(): Map<String, Long> = presence

    private fun table(kind: String): Collection<RegistryRow> = rows[kind]?.values ?: emptyList()

    fun spaces(): List<SpaceInfo> = table(RowKind.SPACES).mapNotNull { row ->
        val f = row.fields
        val id = f.str("id") ?: row.id
        val path = f.str("path") ?: return@mapNotNull null
        SpaceInfo(id, path, f.str("name"), f.str("deviceId") ?: "")
    }

    fun devices(): List<DeviceInfo> = table(RowKind.DEVICES).mapNotNull { row ->
        val f = row.fields
        DeviceInfo(f.str("id") ?: row.id, f.str("name") ?: "device", f.str("platform") ?: "", f.long("lastSeenAt"))
    }

    /** Join chat rows with their live session-status rows into display sessions. */
    fun sessions(): List<SessionSummary> {
        val statuses = table(RowKind.SESSIONS).associateBy({ it.fields.str("chatId") ?: it.id }, { it })
        return table(RowKind.CHATS).mapNotNull { row ->
            val f = row.fields
            val chatId = f.str("id") ?: row.id
            val config = f.obj("config")
            val statusRow = statuses[chatId]?.fields
            val status = when (statusRow?.str("status")) {
                "working" -> SessionStatus.working
                "awaitingInput" -> SessionStatus.awaitingInput
                "errored" -> SessionStatus.errored
                "idle" -> SessionStatus.idle
                else -> SessionStatus.idle
            }
            SessionSummary(
                chatId = chatId,
                title = f.str("title")?.takeIf { it.isNotEmpty() } ?: "New session",
                harness = config?.str("harness"),
                model = config?.str("model"),
                reasoning = config?.str("reasoning"),
                branch = f.str("branch"),
                cwd = f.str("cwd"),
                spaceId = f.str("spaceId"),
                deviceId = f.str("deviceId"),
                archived = f.bool("archived"),
                lastMessagePreview = f.str("lastMessagePreview"),
                lastMessageAt = f.long("lastMessageAt"),
                createdAt = f.long("createdAt") ?: 0,
                lastSeenAt = f.long("lastSeenAt"),
                status = status,
                statusUpdatedAt = statusRow?.long("updatedAt"),
            )
        }
    }

    private fun nextHlc(): String {
        val now = System.currentTimeMillis()
        if (now > lastMs) { lastMs = now; counter = 0 } else {
            counter += 1
            if (counter > 999_999) { lastMs += 1; counter = 0 }
        }
        return "%013d-%06d-%s".format(lastMs, counter, connection.deviceId)
    }

    /** Push one op batch (LWW; replays are no-ops). Used for the seen marker. */
    suspend fun push(ops: List<RowOp>): PushAck = withContext(Dispatchers.IO) {
        val batch = "b-${nextHlc()}"
        val payload = json.encodeToString(PushBatch.serializer(), PushBatch(batch, ops))
        val u = url("/registry/${connection.orgId}/push?device=${connection.deviceId}")
        val req = authed(Request.Builder().url(u))
            .post(payload.toRequestBody("application/json".toMediaType()))
            .build()
        http.newCall(req).execute().use { r ->
            val body = r.body?.string().orEmpty()
            if (!r.isSuccessful) throw CosmosException("push ${r.code}: $body")
            json.decodeFromString(PushAck.serializer(), body)
        }
    }

    /** Mark a chat seen (LWW `lastSeenAt` on the chat row) — matches
     *  RegistryDoc.set_chat_seen. */
    suspend fun markSeen(chatId: String) {
        val hlc = nextHlc()
        val set = mapOf("lastSeenAt" to JsonPrimitive(System.currentTimeMillis()))
        push(listOf(RowOp(kind = RowKind.CHATS, id = chatId, op = "update", set = set, hlc = hlc)))
    }

    /** POST /device/{host}/nudge {chatId} — wake a cold host to drain commands. */
    suspend fun nudge(deviceId: String, chatId: String): Unit = withContext(Dispatchers.IO) {
        val body = buildJsonObject { put("chatId", chatId) }.toString()
        val req = authed(Request.Builder().url(url("/device/$deviceId/nudge")))
            .post(body.toRequestBody("application/json".toMediaType()))
            .build()
        runCatching { http.newCall(req).execute().use { } }
    }
}

class CosmosException(message: String) : Exception(message)
