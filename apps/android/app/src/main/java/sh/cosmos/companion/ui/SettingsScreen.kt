package sh.cosmos.companion.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.cosmos.companion.net.Connection
import sh.cosmos.companion.theme.Geist
import sh.cosmos.companion.theme.LocalCosmos
import sh.cosmos.companion.theme.ThemeMode
import sh.cosmos.companion.vm.LinkState

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    connection: Connection?,
    link: LinkState,
    theme: ThemeMode,
    onTheme: (ThemeMode) -> Unit,
    onReconnect: () -> Unit,
    onDisconnect: () -> Unit,
    onBack: () -> Unit,
) {
    val cosmos = LocalCosmos.current
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Settings", fontFamily = Geist, fontWeight = FontWeight.SemiBold) },
                navigationIcon = {
                    IconButton(onClick = onBack, modifier = Modifier.semantics { contentDescription = "Back" }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
        containerColor = MaterialTheme.colorScheme.background,
    ) { insets ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(insets)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp, vertical = 12.dp),
        ) {
            Text("Appearance", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(8.dp))
            val modes = ThemeMode.entries
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                modes.forEachIndexed { index, mode ->
                    SegmentedButton(
                        selected = theme == mode,
                        onClick = { onTheme(mode) },
                        shape = SegmentedButtonDefaults.itemShape(index, modes.size),
                        label = { Text(mode.name, fontFamily = Geist, fontSize = 13.sp) },
                    )
                }
            }
            Spacer(Modifier.height(28.dp))
            Text("Connection", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(8.dp))
            Card(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(16.dp),
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
            ) {
                Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                    InfoRow("Status", when (link) {
                        LinkState.Online -> "Online"
                        LinkState.Reconnecting -> "Reconnecting"
                        LinkState.Offline -> "Offline"
                    })
                    InfoRow("Edge", connection?.edgeUrl ?: "—")
                    InfoRow("User", connection?.userId ?: "—")
                    InfoRow("Org", connection?.orgId ?: "—")
                    InfoRow("Device", connection?.deviceName ?: "Cosmos Android")
                }
            }
            Spacer(Modifier.height(16.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Button(onClick = onReconnect, modifier = Modifier.weight(1f).height(48.dp)) {
                    Text("Reconnect", fontFamily = Geist, fontWeight = FontWeight.SemiBold)
                }
                Button(
                    onClick = onDisconnect,
                    modifier = Modifier.weight(1f).height(48.dp),
                    colors = ButtonDefaults.buttonColors(containerColor = cosmos.danger, contentColor = MaterialTheme.colorScheme.onError),
                ) {
                    Text("Disconnect", fontFamily = Geist, fontWeight = FontWeight.SemiBold)
                }
            }
            Spacer(Modifier.height(32.dp))
            Text("About", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(8.dp))
            Text(
                "Cosmos companion 0.2.77. Native Android client for the same registry + chat2 tail protocol the desktop and iOS apps use. Durable send/interrupt needs the native Loro bridge.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontFamily = Geist,
                fontSize = 13.sp,
                lineHeight = 20.sp,
            )
        }
    }
}

@Composable
private fun InfoRow(label: String, value: String) {
    Column(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
        Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, fontFamily = Geist, fontSize = 12.sp)
        Text(value, fontFamily = Geist, fontSize = 15.sp)
    }
}
