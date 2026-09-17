package sh.cosmos.companion.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Logout
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.cosmos.companion.net.SessionSummary
import sh.cosmos.companion.theme.CosmosColors
import sh.cosmos.companion.theme.Geist
import sh.cosmos.companion.vm.Workspace

@Composable
fun SessionsScreen(
    workspace: Workspace,
    onOpen: (String) -> Unit,
    onRefresh: () -> Unit,
    onDisconnect: () -> Unit,
) {
    val now = System.currentTimeMillis()
    val active = workspace.sessions.filter { !it.archived }
    val onlineDevices = workspace.devices.count { it.online(now, workspace.presence) }

    Column(Modifier.fillMaxSize().background(CosmosColors.bg)) {
        // Header
        Row(
            Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp, top = 18.dp, bottom = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                Text("Cosmos", fontFamily = Geist, fontWeight = FontWeight.SemiBold, fontSize = 22.sp, color = CosmosColors.text)
                Text(
                    "${active.size} sessions · $onlineDevices device${if (onlineDevices == 1) "" else "s"} online",
                    color = CosmosColors.textFaint, fontSize = 12.sp,
                )
            }
            IconButton(onClick = onRefresh) {
                Icon(Icons.Filled.Refresh, "Refresh", tint = CosmosColors.textMuted)
            }
            IconButton(onClick = onDisconnect) {
                Icon(Icons.Filled.Logout, "Disconnect", tint = CosmosColors.textMuted)
            }
        }

        if (active.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(
                    "No sessions yet.\nStart one on your desktop and it appears here.",
                    color = CosmosColors.textFaint, fontSize = 14.sp,
                )
            }
        } else {
            LazyColumn(Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 24.dp)) {
                items(active, key = { it.chatId }) { s ->
                    SessionRow(s, now) { onOpen(s.chatId) }
                }
            }
        }
    }
}

@Composable
private fun SessionRow(s: SessionSummary, now: Long, onClick: () -> Unit) {
    val indicator = s.indicator(now)
    Column(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            StatusDot(indicator)
            Spacer(Modifier.width(10.dp))
            Text(
                s.title,
                color = CosmosColors.text,
                fontWeight = if (s.unseen) FontWeight.SemiBold else FontWeight.Medium,
                fontSize = 15.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(8.dp))
            Text(indicator.label(), color = indicator.color(), fontSize = 11.sp, fontWeight = FontWeight.Medium)
        }
        if (!s.lastMessagePreview.isNullOrBlank()) {
            Spacer(Modifier.height(4.dp))
            Text(
                s.lastMessagePreview,
                color = CosmosColors.textMuted, fontSize = 13.sp,
                maxLines = 1, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(start = 18.dp),
            )
        }
        Spacer(Modifier.height(6.dp))
        Row(Modifier.padding(start = 18.dp), verticalAlignment = Alignment.CenterVertically) {
            s.harness?.let { Pill(harnessLabel(it)); Spacer(Modifier.width(6.dp)) }
            s.model?.let { Pill(it, color = CosmosColors.textFaint); Spacer(Modifier.width(6.dp)) }
            s.branch?.let {
                Text(it, color = CosmosColors.textFaint, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
    Box(Modifier.fillMaxWidth().height(1.dp).padding(start = 20.dp).background(CosmosColors.border))
}

fun harnessLabel(id: String): String = when (id) {
    "claude-code" -> "Claude Code"
    "codex" -> "Codex"
    "cursor" -> "Cursor"
    "devin" -> "Devin"
    "grok" -> "Grok"
    "hermes" -> "Hermes"
    "pi" -> "Pi"
    "opencode" -> "OpenCode"
    "antigravity", "agy" -> "Antigravity"
    "amp" -> "Amp"
    "mock" -> "Mock"
    else -> id.replaceFirstChar { it.uppercase() }
}
