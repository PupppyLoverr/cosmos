package sh.cosmos.companion

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import sh.cosmos.companion.net.RegistryBridge
import sh.cosmos.companion.theme.CosmosTheme
import sh.cosmos.companion.ui.ConnectScreen
import sh.cosmos.companion.ui.SessionScreen
import sh.cosmos.companion.ui.SessionsScreen
import sh.cosmos.companion.ui.SettingsScreen
import sh.cosmos.companion.vm.AppViewModel
import sh.cosmos.companion.vm.ConnState

@Composable
fun CosmosApp() {
    val vm: AppViewModel = viewModel()
    val theme by vm.theme.collectAsState()
    CosmosTheme(mode = theme) {
        Surface(Modifier.fillMaxSize()) {
            val nav = rememberNavController()
            val state by vm.state.collectAsState()
            val workspace by vm.workspace.collectAsState()
            val link by vm.link.collectAsState()
            val search by vm.search.collectAsState()
            val bridge = remember { RegistryBridge() }
            val saved = remember { vm.savedConnection }

            NavHost(navController = nav, startDestination = "connect") {
                composable("connect") {
                    LaunchedEffect(state) {
                        if (state is ConnState.Connected) {
                            nav.navigate("sessions") { popUpTo("connect") { inclusive = true } }
                        }
                    }
                    ConnectScreen(
                        state = state,
                        defaultEdge = saved?.edgeUrl ?: "http://10.0.2.2:27640",
                        defaultUser = saved?.userId ?: "",
                        defaultOrg = saved?.orgId ?: "",
                        onConnect = { edge, user, org -> vm.connect(edge, user, org, remember = true) },
                    )
                }
                composable("sessions") {
                    SessionsScreen(
                        workspace = workspace,
                        link = link,
                        search = search,
                        onSearch = vm::setSearch,
                        onOpen = { nav.navigate("session/$it") },
                        onRefresh = { vm.refreshNow() },
                        onSettings = { nav.navigate("settings") },
                        loading = state is ConnState.Connecting,
                    )
                }
                composable("settings") {
                    SettingsScreen(
                        connection = vm.client?.conn ?: saved,
                        link = link,
                        theme = theme,
                        onTheme = vm::setTheme,
                        onReconnect = { vm.reconnect() },
                        onDisconnect = {
                            vm.disconnect()
                            nav.navigate("connect") { popUpTo("sessions") { inclusive = true } }
                        },
                        onBack = { nav.popBackStack() },
                    )
                }
                composable("session/{chatId}") { entry ->
                    val chatId = entry.arguments?.getString("chatId").orEmpty()
                    val session = vm.session(chatId)
                    if (session == null) {
                        LaunchedEffect(chatId) { nav.popBackStack() }
                    } else {
                        SessionScreen(
                            session = session,
                            client = vm.client,
                            bridge = bridge,
                            link = link,
                            onBack = { nav.popBackStack() },
                            onReconnect = { vm.reconnect() },
                        )
                    }
                }
            }
        }
    }
}
