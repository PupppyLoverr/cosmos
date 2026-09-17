package sh.cosmos.companion.vm

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import sh.cosmos.companion.data.Prefs
import sh.cosmos.companion.net.Connection
import sh.cosmos.companion.net.CosmosClient
import sh.cosmos.companion.net.DeviceInfo
import sh.cosmos.companion.net.SessionSummary
import sh.cosmos.companion.net.SpaceInfo

sealed interface ConnState {
    data object Disconnected : ConnState
    data object Connecting : ConnState
    data class Connected(val authInfo: String) : ConnState
    data class Failed(val message: String) : ConnState
}

data class Workspace(
    val sessions: List<SessionSummary> = emptyList(),
    val spaces: List<SpaceInfo> = emptyList(),
    val devices: List<DeviceInfo> = emptyList(),
    val presence: Map<String, Long> = emptyMap(),
    val lastSync: Long = 0,
)

/**
 * Owns the edge connection lifecycle and the polled workspace mirror. Sessions
 * are refreshed on a light interval (no aggressive polling) — the registry
 * delta pull is cheap and only ships changed rows.
 */
class AppViewModel(app: Application) : AndroidViewModel(app) {
    private val prefs = Prefs(app)

    private val _state = MutableStateFlow<ConnState>(ConnState.Disconnected)
    val state: StateFlow<ConnState> = _state.asStateFlow()

    private val _workspace = MutableStateFlow(Workspace())
    val workspace: StateFlow<Workspace> = _workspace.asStateFlow()

    var client: CosmosClient? = null
        private set

    private var pollJob: Job? = null

    val savedConnection: Connection? get() = prefs.loadConnection()

    fun connect(edgeUrl: String, userId: String, orgId: String, remember: Boolean) {
        val normalizedUser = userId.trim()
        val normalizedOrg = orgId.trim()
        // Dev-mode bearer is literally `user@org` (apps/ios AppConfig / edge auth).
        val token = "$normalizedUser@$normalizedOrg"
        val connection = Connection(
            edgeUrl = edgeUrl.trim(),
            userId = normalizedUser,
            orgId = normalizedOrg,
            token = token,
            deviceId = prefs.deviceId,
            deviceName = "Cosmos Android",
            mode = "dev",
        )
        connectWith(connection, remember)
    }

    fun connectWith(connection: Connection, remember: Boolean) {
        pollJob?.cancel()
        _state.value = ConnState.Connecting
        val c = CosmosClient(connection)
        client = c
        viewModelScope.launch {
            try {
                val health = runCatching { c.health() }.getOrDefault("")
                c.pull()
                refreshFromClient(c)
                if (remember) prefs.saveConnection(connection)
                _state.value = ConnState.Connected(authInfo = health)
                startPolling(c)
            } catch (e: Exception) {
                _state.value = ConnState.Failed(e.message ?: "Connection failed")
            }
        }
    }

    private fun startPolling(c: CosmosClient) {
        pollJob = viewModelScope.launch {
            while (true) {
                delay(3_000)
                try {
                    c.pull()
                    refreshFromClient(c)
                } catch (_: Exception) {
                    // Transient; keep the last good mirror and retry.
                }
            }
        }
    }

    fun refreshNow() {
        val c = client ?: return
        viewModelScope.launch {
            runCatching { c.pull(); refreshFromClient(c) }
        }
    }

    private fun refreshFromClient(c: CosmosClient) {
        _workspace.value = Workspace(
            sessions = c.sessions().sortedByDescending { it.lastMessageAt ?: it.createdAt },
            spaces = c.spaces(),
            devices = c.devices(),
            presence = c.currentPresence(),
            lastSync = System.currentTimeMillis(),
        )
    }

    fun session(chatId: String): SessionSummary? =
        _workspace.value.sessions.firstOrNull { it.chatId == chatId }

    fun markSeen(chatId: String) {
        val c = client ?: return
        viewModelScope.launch { runCatching { c.markSeen(chatId) } }
    }

    fun disconnect() {
        pollJob?.cancel()
        pollJob = null
        client = null
        prefs.clear()
        _workspace.value = Workspace()
        _state.value = ConnState.Disconnected
    }
}
