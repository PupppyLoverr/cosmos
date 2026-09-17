package sh.cosmos.companion.ui

import androidx.compose.animation.core.withInfiniteAnimationFrameMillis
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import sh.cosmos.companion.net.ChatBridge
import sh.cosmos.companion.net.ChatIndicator
import sh.cosmos.companion.net.CosmosClient
import sh.cosmos.companion.net.SendOutcome
import sh.cosmos.companion.net.SessionSummary
import sh.cosmos.companion.net.TranscriptItem
import sh.cosmos.companion.theme.CosmosColors
import sh.cosmos.companion.theme.Geist
import sh.cosmos.companion.theme.HarnessMark
import sh.cosmos.companion.theme.Motion
import sh.cosmos.companion.theme.WorkingSpinner

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
                add(TranscriptItem("preview", "assistant", session.lastMessagePreview!!, session.lastMessageAt ?: now))
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
            Modifier.fillMaxWidth().padding(start = 2.dp, end = 16.dp, top = 10.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = CosmosColors.textMuted) }
            Column(Modifier.weight(1f)) {
                Text(session.title, color = CosmosColors.text, fontFamily = Geist, fontWeight = FontWeight.SemiBold,
                    fontSize = 16.sp, maxLines = 1)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    StatusCorner(indicator)
                    session.branch?.takeIf { it.isNotBlank() }?.let {
                        Text("  ·  $it", color = CosmosColors.textFaint, fontFamily = Geist, fontSize = 12.sp, maxLines = 1)
                    }
                }
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(CosmosColors.border))

        // Transcript
        LazyColumn(
            state = scrollState,
            modifier = Modifier.weight(1f).fillMaxWidth(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 16.dp, vertical = 18.dp),
            verticalArrangement = Arrangement.spacedBy(18.dp),
        ) {
            if (items.isEmpty()) {
                item {
                    Text(
                        "Live status and the latest message sync from the registry. The full " +
                            "transcript streams over chat2 with the native bridge.",
                        color = CosmosColors.textFaint, fontFamily = Geist, fontSize = 13.sp,
                    )
                }
            }
            items(items, key = { it.id }) { MessageBlock(it) }
            if (indicator == ChatIndicator.working) {
                item { WorkingRow(session.chatId, session.statusUpdatedAt ?: now) }
            }
        }

        note?.let {
            Text(it, color = CosmosColors.textFaint, fontFamily = Geist, fontSize = 11.sp,
                modifier = Modifier.padding(horizontal = 24.dp, vertical = 2.dp))
        }

        // Meta chips
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())
                .padding(horizontal = 16.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            session.model?.let { ModelChip(session.harness, it) }
            session.reasoning?.let { Pill(it, color = CosmosColors.textFaint) }
        }

        Composer(
            draft = draft,
            onDraft = { draft = it },
            working = indicator == ChatIndicator.working,
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
private fun ModelChip(harness: String?, model: String) {
    Row(
        Modifier.clip(RoundedCornerShape(50)).background(CosmosColors.elementHover)
            .border(1.dp, CosmosColors.border, RoundedCornerShape(50))
            .padding(horizontal = 9.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        harness?.let { HarnessMark(it, size = 12.dp, neutral = CosmosColors.textMuted); Spacer(Modifier.width(6.dp)) }
        Text(model, color = CosmosColors.textMuted, fontFamily = Geist, fontWeight = FontWeight.Medium, fontSize = 12.sp)
    }
}

@Composable
private fun MessageBlock(item: TranscriptItem) {
    if (item.role == "user") {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            Box(
                Modifier.widthIn(max = 300.dp)
                    .clip(RoundedCornerShape(22.dp))
                    .background(Color.White.copy(alpha = 0.07f))
                    .padding(horizontal = 16.dp, vertical = 11.dp),
            ) {
                Text(item.text, color = CosmosColors.text, fontFamily = Geist, fontSize = 16.sp, lineHeight = 24.sp)
            }
        }
    } else {
        // Assistant / system: plain body text, no bubble (matches desktop/iOS transcript).
        Text(item.text, color = CosmosColors.text.copy(alpha = 0.92f), fontFamily = Geist, fontSize = 16.sp, lineHeight = 24.sp)
    }
}

@Composable
private fun WorkingRow(chatId: String, startedAt: Long) {
    val seed = remember(chatId) { Motion.flavourSeed(chatId) }
    val elapsed by produceState(0L) {
        while (true) { withInfiniteAnimationFrameMillis { value = (System.currentTimeMillis() - startedAt) / 1000 } }
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        WorkingSpinner()
        Spacer(Modifier.width(10.dp))
        Text(Motion.flavourWord(seed, elapsed), color = CosmosColors.textMuted, fontFamily = Geist,
            fontWeight = FontWeight.Medium, fontSize = 14.sp)
        Spacer(Modifier.width(8.dp))
        Text(Motion.formatElapsed(elapsed), color = CosmosColors.textFaint, fontFamily = Geist, fontSize = 12.sp)
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
    val hasContent = draft.isNotBlank()
    val buttonActive = hasContent || (working && !hasContent)
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // Glass pill
        Row(
            Modifier.weight(1f)
                .clip(RoundedCornerShape(24.dp))
                .background(Color.White.copy(alpha = 0.04f))
                .border(1.dp, Color.White.copy(alpha = 0.06f), RoundedCornerShape(24.dp))
                .padding(start = 16.dp, end = 6.dp, top = 5.dp, bottom = 5.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.weight(1f).padding(vertical = 8.dp)) {
                if (draft.isEmpty()) {
                    Text("Message", color = CosmosColors.text.copy(alpha = 0.3f), fontFamily = Geist, fontSize = 16.sp)
                }
                BasicTextField(
                    value = draft,
                    onValueChange = onDraft,
                    textStyle = TextStyle(color = CosmosColors.text, fontFamily = Geist, fontSize = 16.sp, lineHeight = 22.sp),
                    cursorBrush = SolidColor(CosmosColors.accent),
                    maxLines = 6,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
        Spacer(Modifier.width(8.dp))
        // Send / Stop circle
        Box(
            Modifier.size(44.dp).clip(CircleShape)
                .background(if (buttonActive) CosmosColors.text else Color.White.copy(alpha = 0.10f))
                .clickable(enabled = buttonActive) { if (working && !hasContent) onStop() else onSend() },
            contentAlignment = Alignment.Center,
        ) {
            if (working && !hasContent) {
                Box(Modifier.size(12.dp).clip(RoundedCornerShape(3.5.dp)).background(CosmosColors.bg))
            } else {
                Icon(Icons.Filled.ArrowUpward, "Send",
                    tint = if (buttonActive) CosmosColors.bg else CosmosColors.textFaint,
                    modifier = Modifier.size(18.dp))
            }
        }
    }
}

