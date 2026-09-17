package sh.cosmos.companion.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import sh.cosmos.companion.net.ChatBridge
import sh.cosmos.companion.net.CosmosClient
import sh.cosmos.companion.net.SendOutcome
import sh.cosmos.companion.net.SessionStatus
import sh.cosmos.companion.net.SessionSummary
import sh.cosmos.companion.net.TranscriptItem
import sh.cosmos.companion.theme.CosmosColors
import sh.cosmos.companion.theme.Geist
import sh.cosmos.companion.theme.GeistMono

@Composable
fun SessionScreen(
    session: SessionSummary,
    client: CosmosClient?,
    bridge: ChatBridge,
    onBack: () -> Unit,
) {
    val now = System.currentTimeMillis()
    val indicator = session.indicator(now)
    val scope = rememberCoroutineScope()
    val scrollState = rememberLazyListState()

    val items = remember(session.chatId) {
        mutableStateListOf<TranscriptItem>().apply {
            if (!session.lastMessagePreview.isNullOrBlank()) {
                add(TranscriptItem("preview", "assistant", session.lastMessagePreview, session.lastMessageAt ?: now))
            }
        }
    }
    var draft by remember(session.chatId) { mutableStateOf("") }
    var note by remember(session.chatId) { mutableStateOf<String?>(null) }

    LaunchedEffect(session.chatId) { client?.let { runCatching { it.markSeen(session.chatId) } } }
    LaunchedEffect(items.size) { if (items.isNotEmpty()) scrollState.animateScrollToItem(items.size - 1) }

    Column(Modifier.fillMaxSize().background(CosmosColors.bg).imePadding()) {
        // Header
        Row(
            Modifier.fillMaxWidth().padding(start = 4.dp, end = 16.dp, top = 12.dp, bottom = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) {
                Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = CosmosColors.textMuted)
            }
            Column(Modifier.weight(1f)) {
                Text(session.title, color = CosmosColors.text, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, maxLines = 1)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    StatusDot(indicator)
                    Spacer(Modifier.width(6.dp))
                    Text(
                        buildString {
                            append(indicator.label())
                            session.branch?.let { append("  ·  $it") }
                        },
                        color = CosmosColors.textFaint, fontSize = 12.sp, maxLines = 1,
                    )
                }
            }
        }
        MetaBar(session)
        Box(Modifier.fillMaxWidth().height(1.dp).background(CosmosColors.border))

        // Transcript
        LazyColumn(
            state = scrollState,
            modifier = Modifier.weight(1f).fillMaxWidth(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (items.isEmpty()) {
                item {
                    Text(
                        "No messages mirrored yet. The latest message preview and live status " +
                            "sync from the registry; the full transcript streams over chat2 with the native bridge.",
                        color = CosmosColors.textFaint, fontSize = 13.sp,
                    )
                }
            }
            items(items, key = { it.id }) { MessageBubble(it) }
        }

        note?.let {
            Text(it, color = CosmosColors.textFaint, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
        }

        // Composer
        Composer(
            draft = draft,
            onDraft = { draft = it },
            working = indicator == sh.cosmos.companion.net.ChatIndicator.working,
            onSend = {
                val text = draft.trim()
                if (text.isEmpty()) return@Composer
                items.add(TranscriptItem("u-${System.currentTimeMillis()}", "user", text, System.currentTimeMillis()))
                draft = ""
                val c = client
                if (c != null) scope.launch {
                    when (val r = bridge.send(c, session, text)) {
                        is SendOutcome.Delivered -> note = null
                        is SendOutcome.Queued -> note = r.note
                        is SendOutcome.Failed -> note = r.message
                    }
                }
            },
            onStop = {
                val c = client ?: return@Composer
                scope.launch {
                    when (val r = bridge.stop(c, session)) {
                        is SendOutcome.Queued -> note = r.note
                        is SendOutcome.Failed -> note = r.message
                        else -> {}
                    }
                }
            },
        )
    }
}

@Composable
private fun MetaBar(s: SessionSummary) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        s.harness?.let { Pill(harnessLabel(it)); Spacer(Modifier.width(6.dp)) }
        s.model?.let { Pill(it, color = CosmosColors.textFaint); Spacer(Modifier.width(6.dp)) }
        s.reasoning?.let { Pill(it, color = CosmosColors.textFaint) }
    }
}

@Composable
private fun MessageBubble(item: TranscriptItem) {
    val isUser = item.role == "user"
    Row(Modifier.fillMaxWidth(), horizontalArrangement = if (isUser) Arrangement.End else Arrangement.Start) {
        Box(
            Modifier
                .clip(RoundedCornerShape(if (isUser) 18.dp else 10.dp))
                .background(if (isUser) CosmosColors.elementActive else CosmosColors.surface)
                .border(1.dp, CosmosColors.border, RoundedCornerShape(if (isUser) 18.dp else 10.dp))
                .padding(horizontal = 14.dp, vertical = 10.dp),
        ) {
            Text(
                item.text,
                color = if (isUser) CosmosColors.text else CosmosColors.textMuted,
                fontSize = 15.sp,
                fontFamily = Geist,
            )
        }
    }
}

@Composable
private fun Composer(
    draft: String,
    onDraft: (String) -> Unit,
    working: Boolean,
    onSend: () -> Unit,
    onStop: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlinedTextField(
            value = draft,
            onValueChange = onDraft,
            placeholder = { Text("Do anything…", color = CosmosColors.textFaint) },
            modifier = Modifier.weight(1f),
            shape = RoundedCornerShape(22.dp),
            maxLines = 5,
            colors = OutlinedTextFieldDefaults.colors(
                focusedTextColor = CosmosColors.text,
                unfocusedTextColor = CosmosColors.text,
                focusedBorderColor = CosmosColors.borderStrong,
                unfocusedBorderColor = CosmosColors.border,
                cursorColor = CosmosColors.accent,
                focusedContainerColor = CosmosColors.surface,
                unfocusedContainerColor = CosmosColors.surface,
            ),
        )
        Spacer(Modifier.width(8.dp))
        val send = draft.isNotBlank()
        IconButton(
            onClick = { if (working && !send) onStop() else onSend() },
            modifier = Modifier
                .clip(RoundedCornerShape(20.dp))
                .background(if (working && !send) CosmosColors.danger else CosmosColors.accentStrong)
                .height(44.dp).width(44.dp),
        ) {
            if (working && !send) {
                Icon(Icons.Filled.Stop, "Stop", tint = CosmosColors.text)
            } else {
                Icon(Icons.Filled.ArrowUpward, "Send", tint = CosmosColors.text)
            }
        }
    }
}
