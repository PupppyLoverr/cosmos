package sh.cosmos.companion.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.cosmos.companion.theme.CosmosColors
import sh.cosmos.companion.theme.Geist
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

    Column(
        Modifier
            .fillMaxSize()
            .background(CosmosColors.bg)
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 24.dp, vertical = 40.dp),
        horizontalAlignment = Alignment.Start,
        verticalArrangement = Arrangement.Center,
    ) {
        Text("Cosmos", fontFamily = Geist, fontWeight = FontWeight.SemiBold, fontSize = 34.sp, color = CosmosColors.text)
        Spacer(Modifier.height(6.dp))
        Text(
            "Companion for your desktop & cloud agent sessions.",
            color = CosmosColors.textMuted,
            fontSize = 15.sp,
        )
        Spacer(Modifier.height(36.dp))

        FieldLabel("Edge URL")
        CosmosField(edge, { edge = it }, "https://edge.zeron.sh", KeyboardType.Uri, connecting)
        Spacer(Modifier.height(16.dp))

        FieldLabel("User ID")
        CosmosField(user, { user = it }, "alice", KeyboardType.Text, connecting)
        Spacer(Modifier.height(16.dp))

        FieldLabel("Org ID")
        CosmosField(org, { org = it }, "org1", KeyboardType.Text, connecting)
        Spacer(Modifier.height(28.dp))

        Button(
            onClick = { onConnect(edge, user, org) },
            enabled = !connecting && edge.isNotBlank() && user.isNotBlank() && org.isNotBlank(),
            modifier = Modifier.fillMaxWidth().height(52.dp),
            colors = ButtonDefaults.buttonColors(
                containerColor = CosmosColors.accentStrong,
                contentColor = CosmosColors.text,
                disabledContainerColor = CosmosColors.surfaceRaised,
                disabledContentColor = CosmosColors.textFaint,
            ),
        ) {
            if (connecting) {
                CircularProgressIndicator(strokeWidth = 2.dp, color = CosmosColors.text, modifier = Modifier.height(20.dp))
            } else {
                Text("Connect", fontWeight = FontWeight.SemiBold, fontSize = 16.sp)
            }
        }

        if (state is ConnState.Failed) {
            Spacer(Modifier.height(16.dp))
            Text(state.message, color = CosmosColors.danger, fontSize = 13.sp)
        }

        Spacer(Modifier.height(32.dp))
        Text(
            "Dev pairing: point at an AUTH_MODE=dev edge (e.g. wrangler dev). " +
                "The bearer is user@org — the same identity your desktop signs in with. " +
                "Woozlit/WorkOS sign-in shares these room URLs and is a follow-up on this preview build.",
            color = CosmosColors.textFaint,
            fontSize = 12.sp,
        )
    }
}

@Composable
private fun FieldLabel(text: String) {
    Text(text, color = CosmosColors.textMuted, fontSize = 13.sp, fontWeight = FontWeight.Medium)
    Spacer(Modifier.height(6.dp))
}

@Composable
private fun CosmosField(
    value: String,
    onChange: (String) -> Unit,
    placeholder: String,
    keyboard: KeyboardType,
    disabled: Boolean,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        enabled = !disabled,
        singleLine = true,
        placeholder = { Text(placeholder, color = CosmosColors.textFaint) },
        keyboardOptions = KeyboardOptions(keyboardType = keyboard),
        modifier = Modifier.fillMaxWidth(),
        colors = OutlinedTextFieldDefaults.colors(
            focusedTextColor = CosmosColors.text,
            unfocusedTextColor = CosmosColors.text,
            focusedBorderColor = CosmosColors.accent,
            unfocusedBorderColor = CosmosColors.border,
            cursorColor = CosmosColors.accent,
            focusedContainerColor = CosmosColors.surface,
            unfocusedContainerColor = CosmosColors.surface,
        ),
    )
}
