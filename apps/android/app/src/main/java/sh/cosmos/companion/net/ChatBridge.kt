package sh.cosmos.companion.net

/** A rendered transcript line. */
data class TranscriptItem(
    val id: String,
    val role: String, // "user" | "assistant" | "system"
    val text: String,
    val ts: Long,
)

sealed interface SendOutcome {
    data object Delivered : SendOutcome
    data class Queued(val note: String) : SendOutcome
    data class Failed(val message: String) : SendOutcome
}

/**
 * The chat2 session-doc path (transcript + command ledger). The session doc is
 * a Loro CRDT; producing a durable command or materializing the transcript
 * requires a Loro engine, which is provided by the optional native bridge
 * (a UniFFI wrapper over crates/sync + crates/doc, the exact desktop code).
 *
 * When the native bridge is not present in the running build, the companion
 * still shows session state and the latest message from the registry row, and
 * routes a send as a host nudge + seen marker so the desktop opens the chat —
 * clearly surfaced as non-durable until the bridge lands.
 */
interface ChatBridge {
    val durable: Boolean
    suspend fun send(client: CosmosClient, session: SessionSummary, text: String): SendOutcome
    suspend fun stop(client: CosmosClient, session: SessionSummary): SendOutcome
}

/** Registry-only fallback bridge (no Loro). Nudges the host so it surfaces the
 *  chat; delivery of the actual instruction is not durable without the native
 *  bridge. Honest by construction — reports Queued, never Delivered. */
class RegistryBridge : ChatBridge {
    override val durable = false

    override suspend fun send(client: CosmosClient, session: SessionSummary, text: String): SendOutcome {
        runCatching { client.markSeen(session.chatId) }
        session.deviceId?.let { runCatching { client.nudge(it, session.chatId) } }
        val host = session.deviceId ?: "host"
        return SendOutcome.Queued(
            "Nudged $host. Durable delivery needs the Cosmos native bridge (preview).",
        )
    }

    override suspend fun stop(client: CosmosClient, session: SessionSummary): SendOutcome {
        session.deviceId?.let { runCatching { client.nudge(it, session.chatId) } }
        return SendOutcome.Queued("Interrupt requires the native bridge (preview).")
    }
}
