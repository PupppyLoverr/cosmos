package sh.cosmos.companion.net

/** A rendered transcript line. */
data class TranscriptItem(
    val id: String,
    val role: String, // "user" | "assistant" | "system"
    val text: String,
    val ts: Long,
    val streaming: Boolean = false,
)

sealed interface SendOutcome {
    data object Delivered : SendOutcome
    data class Queued(val note: String) : SendOutcome
    data class Failed(val message: String) : SendOutcome
}

/**
 * The chat2 session-doc path (transcript + command ledger). The session doc is
 * a Loro CRDT; producing a durable `run`/`interrupt` command requires a Loro
 * engine (the planned UniFFI wrapper over crates/sync + crates/doc).
 *
 * Transcript **reads** do not need Loro: the host publishes a JSON tail
 * sidecar (`GET /chat2/{id}/tail`) that this bridge polls. Sends still go
 * through the registry nudge so a running desktop host opens the chat;
 * durable delivery is reported honestly as Queued until the native bridge
 * lands.
 */
interface ChatBridge {
    val durable: Boolean
    suspend fun transcript(client: CosmosClient, session: SessionSummary): List<TranscriptItem>
    suspend fun send(client: CosmosClient, session: SessionSummary, text: String): SendOutcome
    suspend fun stop(client: CosmosClient, session: SessionSummary): SendOutcome
}

class RegistryBridge : ChatBridge {
    override val durable = false

    override suspend fun transcript(client: CosmosClient, session: SessionSummary): List<TranscriptItem> {
        val tail = runCatching { client.chatTail(session.chatId) }.getOrNull()
        val fromTail = tail?.messages.orEmpty().mapNotNull { it.toTranscriptItem() }
        if (fromTail.isNotEmpty()) return fromTail
        if (!session.lastMessagePreview.isNullOrBlank()) {
            return listOf(
                TranscriptItem(
                    id = "preview",
                    role = "assistant",
                    text = session.lastMessagePreview,
                    ts = session.lastMessageAt ?: System.currentTimeMillis(),
                ),
            )
        }
        return emptyList()
    }

    override suspend fun send(client: CosmosClient, session: SessionSummary, text: String): SendOutcome {
        runCatching { client.markSeen(session.chatId) }
        val host = session.deviceId
        if (host == null) {
            return SendOutcome.Failed("No host device for this session.")
        }
        return try {
            client.nudge(host, session.chatId)
            SendOutcome.Queued(
                "Host nudged. Durable send needs the Cosmos native chat2 bridge — the desktop host must adopt this turn.",
            )
        } catch (e: Exception) {
            SendOutcome.Failed(e.message ?: "Send failed")
        }
    }

    override suspend fun stop(client: CosmosClient, session: SessionSummary): SendOutcome {
        val host = session.deviceId
            ?: return SendOutcome.Failed("No host device for this session.")
        return try {
            client.nudge(host, session.chatId)
            SendOutcome.Queued("Interrupt requires the native chat2 bridge; the host was nudged.")
        } catch (e: Exception) {
            SendOutcome.Failed(e.message ?: "Stop failed")
        }
    }
}

internal fun TailMessage.toTranscriptItem(): TranscriptItem? {
    val body = parts.mapNotNull { part ->
        when (part.kind) {
            "text" -> part.text?.takeIf { it.isNotBlank() }
            "reasoning" -> part.reasoning?.takeIf { it.isNotBlank() }?.let { "Thinking: $it" }
            else -> part.text?.takeIf { it.isNotBlank() }
        }
    }.joinToString("\n").trim()
    if (body.isEmpty()) return null
    val role = when (role.lowercase()) {
        "user" -> "user"
        "system" -> "system"
        else -> "assistant"
    }
    return TranscriptItem(
        id = id.ifBlank { "m-$createdAt" },
        role = role,
        text = body,
        ts = createdAt,
        streaming = status.equals("streaming", ignoreCase = true) || status.equals("running", ignoreCase = true),
    )
}
