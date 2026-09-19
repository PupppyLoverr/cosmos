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
    private val rows: MutableMap<String, MutableMap<String, RegistryRow>> = HashMap()
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
            Registry.applyDelta(rows, resp)
            cursor = resp.seq
            presence = resp.presence
        }
    }

    fun currentPresence(): Map<String, Long> = presence

    fun spaces(): List<SpaceInfo> = Registry.spaces(rows)

    fun devices(): List<DeviceInfo> = Registry.devices(rows)

    /** Join chat rows with their live session-status rows into display sessions. */
    fun sessions(): List<SessionSummary> = Registry.sessions(rows)

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
        http.newCall(req).execute().use { r ->
            if (!r.isSuccessful && r.code !in 400..499) {
                throw CosmosException("nudge ${r.code}")
            }
        }
    }

    /**
     * GET /chat2/{chatId}/tail — host-published last-N transcript JSON
     * (crates/engine chat2_maintenance). Same protocol iOS uses for instant-open.
     * Returns null when the host has not published a sidecar yet (404/empty).
     */
    suspend fun chatTail(chatId: String): SessionTail? = withContext(Dispatchers.IO) {
        val req = authed(Request.Builder().url(url("/chat2/$chatId/tail"))).build()
        http.newCall(req).execute().use { r ->
            if (r.code == 404) return@withContext null
            val body = r.body?.string().orEmpty()
            if (!r.isSuccessful) throw CosmosException("tail ${r.code}: $body")
            if (body.isBlank()) return@withContext null
            json.decodeFromString(SessionTail.serializer(), body)
        }
    }
}

class CosmosException(message: String) : Exception(message)
