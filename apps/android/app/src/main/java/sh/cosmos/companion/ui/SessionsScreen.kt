package sh.cosmos.companion.ui

import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.cosmos.companion.net.ChatIndicator
import sh.cosmos.companion.net.SessionSummary
import sh.cosmos.companion.theme.GitBranchIcon
import sh.cosmos.companion.theme.Geist
import sh.cosmos.companion.theme.HarnessMark
import sh.cosmos.companion.theme.LocalCosmos
import sh.cosmos.companion.theme.MiniSpinner
import sh.cosmos.companion.vm.LinkState
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

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionsScreen(
    workspace: Workspace,
    link: LinkState,
    search: String,
    onSearch: (String) -> Unit,
    onOpen: (String) -> Unit,
    onRefresh: () -> Unit,
    onSettings: () -> Unit,
    loading: Boolean = false,
) {
    val now = System.currentTimeMillis()
    val cosmos = LocalCosmos.current
    var filter by remember { mutableStateOf(SessionFilter.ALL) }
    var refreshing by remember { mutableStateOf(false) }

    val active = workspace.sessions.filter { !it.archived }
    val queried = if (search.isBlank()) active else {
        val q = search.trim().lowercase()
        active.filter { s ->
            listOfNotNull(s.title, s.branch, s.model, s.harness, s.lastMessagePreview, s.cwd)
                .any { it.lowercase().contains(q) }
        }
    }
    val visible = queried.filter { filter.includes(it, now) }
    val onlineDevices = workspace.devices.count { it.online(now, workspace.presence) }
    fun deviceName(id: String?): String =
        workspace.devices.firstOrNull { it.id == id }?.name ?: (id?.take(6) ?: "device")
    fun spaceName(s: SessionSummary): String = when {
        s.spaceId == null -> "No project"
        else -> workspace.spaces.firstOrNull { it.id == s.spaceId }?.displayName
            ?: s.cwd?.substringAfterLast('/') ?: "?"
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text("Sessions", fontFamily = Geist, fontWeight = FontWeight.SemiBold)
                        Text(
                            "${active.size} · $onlineDevices device${if (onlineDevices == 1) "" else "s"} online",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                },
                actions = {
                    IconButton(onClick = onSettings, modifier = Modifier.semantics { contentDescription = "Settings" }) {
                        Icon(Icons.Outlined.Settings, contentDescription = "Settings")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
        containerColor = MaterialTheme.colorScheme.background,
    ) { insets ->
        Column(Modifier.fillMaxSize().padding(insets)) {
            ConnectionBanner(link, onRetry = onRefresh)
            OutlinedTextField(
                value = search,
                onValueChange = onSearch,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 8.dp)
                    .semantics { contentDescription = "Search sessions" },
                singleLine = true,
                leadingIcon = { Icon(Icons.Outlined.Search, contentDescription = null) },
                placeholder = { Text("Search title, branch, model") },
                shape = RoundedCornerShape(16.dp),
                colors = OutlinedTextFieldDefaults.colors(
                    focusedBorderColor = cosmos.accent,
                    cursorColor = cosmos.accent,
                ),
            )
            LazyRow(
                modifier = Modifier.fillMaxWidth(),
                contentPadding = PaddingValues(horizontal = 16.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(SessionFilter.entries.toList()) { f ->
                    val count = queried.count { f.includes(it, now) }
                    FilterChip(
                        selected = filter == f,
                        onClick = { filter = f },
                        label = { Text("${f.title} $count", fontFamily = Geist) },
                        colors = FilterChipDefaults.filterChipColors(
                            selectedContainerColor = cosmos.accent.copy(alpha = 0.18f),
                            selectedLabelColor = cosmos.accent,
                        ),
                    )
                }
            }
            PullToRefreshBox(
                isRefreshing = refreshing,
                onRefresh = {
                    refreshing = true
                    onRefresh()
                    refreshing = false
                },
                modifier = Modifier.weight(1f),
            ) {
                when {
                    loading && active.isEmpty() -> SessionSkeletonList()
                    visible.isEmpty() -> EmptySessions(filter, search.isNotBlank())
                    else -> LazyColumn(
                        Modifier.fillMaxSize(),
                        contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 28.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        items(visible, key = { it.chatId }) { s ->
                            SessionCard(s, now, spaceName(s), deviceName(s.deviceId)) { onOpen(s.chatId) }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun EmptySessions(filter: SessionFilter, searching: Boolean) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.padding(32.dp)) {
            Text(
                when {
                    searching -> "No matching sessions"
                    filter == SessionFilter.ATTENTION -> "You're all caught up"
                    filter == SessionFilter.ALL -> "No sessions yet"
                    else -> "No ${filter.title.lowercase()} sessions"
                },
                style = MaterialTheme.typography.titleMedium,
                fontFamily = Geist,
            )
            Spacer(Modifier.height(6.dp))
            Text(
                "Sessions from your desktop appear here.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontFamily = Geist,
                fontSize = 14.sp,
            )
        }
    }
}

@Composable
private fun SessionSkeletonList() {
    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        repeat(4) {
            Card(
                modifier = Modifier.fillMaxWidth().height(96.dp),
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f)),
            ) {}
        }
    }
}

@Composable
private fun SessionCard(
    s: SessionSummary,
    now: Long,
    space: String,
    device: String,
    onClick: () -> Unit,
) {
    val indicator = s.indicator(now)
    val cosmos = LocalCosmos.current
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .semantics { contentDescription = "Session ${s.title}" },
        shape = RoundedCornerShape(16.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        elevation = CardDefaults.cardElevation(defaultElevation = 1.dp),
    ) {
        Column(Modifier.padding(horizontal = 16.dp, vertical = 14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    "$space · $device",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontFamily = Geist,
                    fontSize = 12.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (indicator == ChatIndicator.idle) {
                    Text(
                        relativeTime(s.lastMessageAt ?: s.createdAt),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontFamily = Geist,
                        fontSize = 12.sp,
                    )
                } else {
                    StatusCorner(indicator)
                }
            }
            Spacer(Modifier.height(4.dp))
            Text(
                s.title,
                fontFamily = Geist,
                fontWeight = FontWeight.SemiBold,
                fontSize = 16.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (!s.lastMessagePreview.isNullOrBlank()) {
                Spacer(Modifier.height(4.dp))
                Text(
                    s.lastMessagePreview.trim(),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontFamily = Geist,
                    fontSize = 14.sp,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Spacer(Modifier.height(8.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                s.harness?.let {
                    HarnessMark(it, size = 12.dp, neutral = cosmos.textMuted)
                    Spacer(Modifier.width(6.dp))
                    Text(harnessLabel(it), color = cosmos.textMuted, fontFamily = Geist, fontSize = 12.sp)
                    Spacer(Modifier.width(8.dp))
                }
                if (!s.branch.isNullOrBlank()) {
                    GitBranchIcon(size = 11.dp, color = cosmos.textMuted)
                    Spacer(Modifier.width(3.dp))
                    Text(s.branch, color = cosmos.textMuted, fontFamily = Geist, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Spacer(Modifier.weight(1f))
                if (indicator == ChatIndicator.working) MiniSpinner()
            }
        }
    }
}
