package sh.cosmos.companion.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.cosmos.companion.net.ChatIndicator
import sh.cosmos.companion.net.SessionSummary
import sh.cosmos.companion.theme.CosmosColors
import sh.cosmos.companion.theme.GitBranchIcon
import sh.cosmos.companion.theme.Geist
import sh.cosmos.companion.theme.HarnessMark
import sh.cosmos.companion.theme.MiniSpinner
import sh.cosmos.companion.vm.Workspace

enum class SessionFilter(val title: String) {
    ALL("All"), ATTENTION("Needs you"), RUNNING("Running"), UNREAD("Unread");

    fun includes(s: SessionSummary, now: Long): Boolean = when (this) {
        ALL -> true
        ATTENTION -> s.indicator(now).let { it == ChatIndicator.awaitingInput || it == ChatIndicator.errored }
        RUNNING -> s.indicator(now) == ChatIndicator.working
        UNREAD -> s.unseen
    }
}

@Composable
fun SessionsScreen(
    workspace: Workspace,
    onOpen: (String) -> Unit,
    onRefresh: () -> Unit,
    onDisconnect: () -> Unit,
) {
    val now = System.currentTimeMillis()
    var filter by remember { mutableStateOf(SessionFilter.ALL) }
    var menuOpen by remember { mutableStateOf(false) }

    val active = workspace.sessions.filter { !it.archived }
    val visible = active.filter { filter.includes(it, now) }
    val onlineDevices = workspace.devices.count { it.online(now, workspace.presence) }
    fun deviceName(id: String?): String =
        workspace.devices.firstOrNull { it.id == id }?.name ?: (id?.take(6) ?: "device")
    fun spaceName(s: SessionSummary): String = when {
        s.spaceId == null -> "No project"
        else -> workspace.spaces.firstOrNull { it.id == s.spaceId }?.displayName
            ?: s.cwd?.substringAfterLast('/') ?: "?"
    }

    Column(Modifier.fillMaxSize().background(CosmosColors.surface)) {
        // Header
        Row(
            Modifier.fillMaxWidth().padding(start = 20.dp, end = 6.dp, top = 20.dp, bottom = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                Text("Cosmos", fontFamily = Geist, fontWeight = FontWeight.SemiBold, fontSize = 24.sp, color = CosmosColors.text)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(5.dp).clip(CircleShape).background(
                        if (onlineDevices > 0) CosmosColors.statusCompleted.copy(alpha = 0.9f) else CosmosColors.textFaint))
                    Spacer(Modifier.width(5.dp))
                    Text(
                        "${active.size} session${if (active.size == 1) "" else "s"} · $onlineDevices device${if (onlineDevices == 1) "" else "s"} online",
                        color = CosmosColors.textFaint, fontFamily = Geist, fontSize = 12.sp,
                    )
                }
            }
            IconButton(onClick = onRefresh) { Icon(Icons.Filled.Refresh, "Refresh", tint = CosmosColors.textMuted) }
            Box {
                IconButton(onClick = { menuOpen = true }) { Icon(Icons.Filled.MoreVert, "Account", tint = CosmosColors.textMuted) }
                DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                    DropdownMenuItem(text = { Text("Refresh") }, onClick = { menuOpen = false; onRefresh() })
                    DropdownMenuItem(text = { Text("Disconnect") }, onClick = { menuOpen = false; onDisconnect() })
                }
            }
        }

        // Filter bar
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())
                .padding(horizontal = 12.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            SessionFilter.entries.forEach { f ->
                val count = active.count { f.includes(it, now) }
                FilterChip(f.title, count, selected = filter == f) { filter = f }
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(CosmosColors.border))

        if (visible.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(
                        if (filter == SessionFilter.ATTENTION) "You're all caught up"
                        else if (filter == SessionFilter.ALL) "Start a conversation"
                        else "No ${filter.title.lowercase()} sessions",
                        color = CosmosColors.text, fontFamily = Geist, fontWeight = FontWeight.Medium, fontSize = 18.sp,
                    )
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "Sessions from your desktop appear here.",
                        color = CosmosColors.textMuted, fontFamily = Geist, fontSize = 14.sp,
                    )
                }
            }
        } else {
            LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 2.dp, bottom = 24.dp)) {
                items(visible, key = { it.chatId }) { s ->
                    ChatRow(s, now, spaceName(s), deviceName(s.deviceId)) { onOpen(s.chatId) }
                }
            }
        }
    }
}

@Composable
private fun FilterChip(title: String, count: Int, selected: Boolean, onClick: () -> Unit) {
    Row(
        Modifier
            .clip(RoundedCornerShape(50))
            .background(if (selected) CosmosColors.surfaceRaised else androidx.compose.ui.graphics.Color.Transparent)
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(title, color = if (selected) CosmosColors.text else CosmosColors.textMuted,
            fontFamily = Geist, fontWeight = FontWeight.Medium, fontSize = 13.sp)
        Text("$count", color = CosmosColors.textFaint, fontFamily = Geist, fontSize = 13.sp)
    }
}

/** The desktop/iOS session row: muted context line + status corner/time; title;
 *  last-message preview; harness mark + branch; mini spinner while Working. */
@Composable
private fun ChatRow(s: SessionSummary, now: Long, space: String, device: String, onClick: () -> Unit) {
    val indicator = s.indicator(now)
    val subline = CosmosColors.textMuted.copy(alpha = 0.5f)
    Column(
        Modifier.fillMaxWidth().clickable(onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 10.dp),
    ) {
        // context line + status corner/time
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("$space @ $device", color = subline, fontFamily = Geist, fontSize = 13.sp,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            Spacer(Modifier.width(8.dp))
            if (indicator == ChatIndicator.idle) {
                Text(relativeTime(s.lastMessageAt ?: s.createdAt), color = subline,
                    fontFamily = Geist, fontWeight = FontWeight.Medium, fontSize = 12.sp)
            } else {
                StatusCorner(indicator)
            }
        }
        Spacer(Modifier.height(2.dp))
        Text(s.title, color = CosmosColors.text, fontFamily = Geist, fontWeight = FontWeight.Medium,
            fontSize = 17.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (!s.lastMessagePreview.isNullOrBlank()) {
            Spacer(Modifier.height(2.dp))
            Text(s.lastMessagePreview.trim(), color = CosmosColors.textMuted.copy(alpha = 0.72f),
                fontFamily = Geist, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Spacer(Modifier.height(6.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            s.harness?.let {
                HarnessMark(it, size = 11.dp, neutral = subline)
                Spacer(Modifier.width(6.dp))
            }
            if (!s.branch.isNullOrBlank()) {
                GitBranchIcon(size = 11.dp, color = subline)
                Spacer(Modifier.width(3.dp))
                Text(s.branch, color = subline, fontFamily = Geist, fontSize = 13.sp,
                    maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
            }
            Spacer(Modifier.weight(1f))
            if (indicator == ChatIndicator.working) MiniSpinner()
        }
    }
    Box(Modifier.fillMaxWidth().height(1.dp).padding(start = 20.dp).background(CosmosColors.border))
}
