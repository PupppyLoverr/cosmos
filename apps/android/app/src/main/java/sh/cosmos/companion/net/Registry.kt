package sh.cosmos.companion.net

/**
 * Pure registry derivation — the row-table semantics shared with
 * crates/doc/src/registry.rs. Kept free of I/O so it is unit-testable against
 * real edge payloads (see SessionLogicTest / RegistryParseTest).
 */
object Registry {
    /** Merge a rows delta into a local replica. `full` replaces wholesale. */
    fun applyDelta(store: MutableMap<String, MutableMap<String, RegistryRow>>, resp: RowsResponse) {
        if (resp.full) store.clear()
        for (row in resp.rows) {
            val table = store.getOrPut(row.kind) { HashMap() }
            if (row.deleted) table.remove(row.id) else table[row.id] = row
        }
    }

    private fun rows(store: Map<String, Map<String, RegistryRow>>, kind: String) =
        store[kind]?.values ?: emptyList()

    fun spaces(store: Map<String, Map<String, RegistryRow>>): List<SpaceInfo> =
        rows(store, RowKind.SPACES).mapNotNull { row ->
            val f = row.fields
            val path = f.str("path") ?: return@mapNotNull null
            SpaceInfo(f.str("id") ?: row.id, path, f.str("name"), f.str("deviceId") ?: "")
        }

    fun devices(store: Map<String, Map<String, RegistryRow>>): List<DeviceInfo> =
        rows(store, RowKind.DEVICES).mapNotNull { row ->
            val f = row.fields
            DeviceInfo(f.str("id") ?: row.id, f.str("name") ?: "device", f.str("platform") ?: "", f.long("lastSeenAt"))
        }

    /** Join chat rows with their live session-status rows into display sessions. */
    fun sessions(store: Map<String, Map<String, RegistryRow>>): List<SessionSummary> {
        val statuses = rows(store, RowKind.SESSIONS).associateBy({ it.fields.str("chatId") ?: it.id }, { it })
        return rows(store, RowKind.CHATS).map { row ->
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
}
