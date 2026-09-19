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
import sh.cosmos.companion.theme.ThemeMode

sealed interface ConnState {
    data object Disconnected : ConnState
    data object Connecting : ConnState
    data class Connected(val authInfo: String) : ConnState
    data class Failed(val message: String) : ConnState
}

enum class LinkState { Online, Reconnecting, Offline }

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
 * delta pull is cheap and only ships changed rows. Failures flip
 * reconnecting → offline instead of silently freezing the last snapshot.
 */
class AppViewModel(app: Application) : AndroidViewModel(app) {
    private val prefs = Prefs(app)

    private val _state = MutableStateFlow<ConnState>(ConnState.Disconnected)
    val state: StateFlow<ConnState> = _state.asStateFlow()

    private val _workspace = MutableStateFlow(Workspace())
    val workspace: StateFlow<Workspace> = _workspace.asStateFlow()

    private val _link = MutableStateFlow(LinkState.Offline)
    val link: StateFlow<LinkState> = _link.asStateFlow()

    private val _search = MutableStateFlow("")
    val search: StateFlow<String> = _search.asStateFlow()

    private val _theme = MutableStateFlow(prefs.themeMode())
    val theme: StateFlow<ThemeMode> = _theme.asStateFlow()

    var client: CosmosClient? = null
        private set

    private var pollJob: Job? = null
    private var failures = 0
    private var lastConnection: Connection? = null

    val savedConnection: Connection? get() = prefs.loadConnection()

    fun setSearch(query: String) {
        _search.value = query
    }

    fun setTheme(mode: ThemeMode) {
        _theme.value = mode
        prefs.saveThemeMode(mode)
    }

    fun connect(edgeUrl: String, userId: String, orgId: String, remember: Boolean) {
        val normalizedUser = userId.trim()
        val normalizedOrg = orgId.trim()
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
        failures = 0
        _state.value = ConnState.Connecting
        _link.value = LinkState.Reconnecting
        val c = CosmosClient(connection)
        client = c
        lastConnection = connection
        viewModelScope.launch {
            try {
                val health = runCatching { c.health() }.getOrDefault("")
                c.pull()
                refreshFromClient(c)
                if (remember) prefs.saveConnection(connection)
                _state.value = ConnState.Connected(authInfo = health)
                _link.value = LinkState.Online
                startPolling(c)
            } catch (e: Exception) {
                _state.value = ConnState.Failed(e.message ?: "Connection failed")
                _link.value = LinkState.Offline
            }
        }
    }

    fun reconnect() {
        val conn = lastConnection ?: prefs.loadConnection() ?: return
        connectWith(conn, remember = true)
    }

    private fun startPolling(c: CosmosClient) {
        pollJob = viewModelScope.launch {
            while (true) {
                delay(if (_link.value == LinkState.Online) 3_000 else 1_500)
                try {
                    c.pull()
                    refreshFromClient(c)
                    failures = 0
                    if (_state.value is ConnState.Connected) _link.value = LinkState.Online
                } catch (_: Exception) {
                    failures += 1
                    _link.value = if (failures >= 4) LinkState.Offline else LinkState.Reconnecting
                }
            }
        }
    }

    fun refreshNow() {
        val c = client ?: return
        viewModelScope.launch {
            try {
                c.pull()
                refreshFromClient(c)
                failures = 0
                _link.value = LinkState.Online
            } catch (_: Exception) {
                _link.value = LinkState.Reconnecting
            }
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
        lastConnection = null
        prefs.clear()
        _workspace.value = Workspace()
        _link.value = LinkState.Offline
        _search.value = ""
        _state.value = ConnState.Disconnected
    }
}
