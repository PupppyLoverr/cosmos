package sh.cosmos.companion.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.cosmos.companion.theme.Geist
import sh.cosmos.companion.theme.LocalCosmos
import sh.cosmos.companion.vm.ConnState

@Composable
fun ConnectScreen(
    state: ConnState,
    defaultEdge: String,
    defaultUser: String,
    defaultOrg: String,
    onConnect: (edge: String, user: String, org: String) -> Unit,
) {
    var edge by rememberSaveable { mutableStateOf(defaultEdge) }
    var user by rememberSaveable { mutableStateOf(defaultUser) }
    var org by rememberSaveable { mutableStateOf(defaultOrg) }
    val connecting = state is ConnState.Connecting
    val cosmos = LocalCosmos.current

    Column(
        Modifier
            .fillMaxSize()
            .imePadding()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 24.dp, vertical = 48.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("Cosmos", fontFamily = Geist, fontWeight = FontWeight.SemiBold, fontSize = 36.sp)
        Spacer(Modifier.height(8.dp))
        Text(
            "Companion for your desktop and cloud agent sessions.",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontFamily = Geist,
            fontSize = 15.sp,
            lineHeight = 22.sp,
        )
        Spacer(Modifier.height(36.dp))

        Field("Edge URL", edge, { edge = it }, "https://edge.zeron.sh", KeyboardType.Uri, connecting, "Edge URL")
        Spacer(Modifier.height(16.dp))
        Field("User ID", user, { user = it }, "alice", KeyboardType.Text, connecting, "User ID")
        Spacer(Modifier.height(16.dp))
        Field("Org ID", org, { org = it }, "org1", KeyboardType.Text, connecting, "Org ID")
        Spacer(Modifier.height(28.dp))

        Button(
            onClick = { onConnect(edge, user, org) },
            enabled = !connecting && edge.isNotBlank() && user.isNotBlank() && org.isNotBlank(),
            modifier = Modifier.fillMaxWidth().height(52.dp).semantics { contentDescription = "Connect" },
        ) {
            if (connecting) {
                CircularProgressIndicator(strokeWidth = 2.dp, modifier = Modifier.height(22.dp), color = MaterialTheme.colorScheme.onPrimary)
            } else {
                Text("Connect", fontWeight = FontWeight.SemiBold, fontSize = 16.sp, fontFamily = Geist)
            }
        }

        if (state is ConnState.Failed) {
            Spacer(Modifier.height(16.dp))
            Text(state.message, color = cosmos.danger, fontFamily = Geist, fontSize = 13.sp)
        }

        Spacer(Modifier.height(32.dp))
        Text(
            "Dev pairing: point at an AUTH_MODE=dev edge. The bearer is user@org — the same identity your desktop signs in with. From the emulator use http://10.0.2.2:<port>.",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontFamily = Geist,
            fontSize = 12.sp,
            lineHeight = 18.sp,
        )
    }
}

@Composable
private fun Field(
    label: String,
    value: String,
    onChange: (String) -> Unit,
    placeholder: String,
    keyboard: KeyboardType,
    disabled: Boolean,
    description: String,
) {
    Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 13.sp, fontWeight = FontWeight.Medium, fontFamily = Geist)
    Spacer(Modifier.height(6.dp))
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        enabled = !disabled,
        singleLine = true,
        placeholder = { Text(placeholder) },
        keyboardOptions = KeyboardOptions(keyboardType = keyboard),
        modifier = Modifier.fillMaxWidth().semantics { contentDescription = description },
    )
}
