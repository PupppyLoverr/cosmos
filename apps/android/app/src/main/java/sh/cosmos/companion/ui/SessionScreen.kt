package sh.cosmos.companion.ui

import androidx.compose.animation.core.withInfiniteAnimationFrameMillis
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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.cosmos.companion.net.ChatBridge
import sh.cosmos.companion.net.ChatIndicator
import sh.cosmos.companion.net.CosmosClient
import sh.cosmos.companion.net.SendOutcome
import sh.cosmos.companion.net.SessionSummary
import sh.cosmos.companion.net.TranscriptItem
import sh.cosmos.companion.theme.Geist
import sh.cosmos.companion.theme.HarnessMark
import sh.cosmos.companion.theme.LocalCosmos
import sh.cosmos.companion.theme.Motion
import sh.cosmos.companion.theme.WorkingSpinner
import sh.cosmos.companion.vm.LinkState

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionScreen(
    session: SessionSummary,
    client: CosmosClient?,
    bridge: ChatBridge,
    link: LinkState = LinkState.Online,
    seedItems: List<TranscriptItem> = emptyList(),
    onBack: () -> Unit,
    onReconnect: () -> Unit = {},
) {
    val now = System.currentTimeMillis()
    val indicator = session.indicator(now)
    val scope = rememberCoroutineScope()
    val scrollState = rememberLazyListState()
    val cosmos = LocalCosmos.current
    val items = remember(session.chatId) { mutableStateListOf<TranscriptItem>() }
    var draft by remember(session.chatId) { mutableStateOf("") }
    var note by remember(session.chatId) { mutableStateOf<String?>(null) }
    var loading by remember(session.chatId) { mutableStateOf(true) }
    var sending by remember { mutableStateOf(false) }

    LaunchedEffect(session.chatId, client, seedItems) {
        client?.let { runCatching { it.markSeen(session.chatId) } }
        if (seedItems.isNotEmpty()) {
            items.clear()
            items.addAll(seedItems)
            loading = false
        } else if (client != null) {
            loading = true
            val loaded = runCatching { bridge.transcript(client, session) }.getOrDefault(emptyList())
            items.clear()
            items.addAll(loaded)
            loading = false
        } else {
            session.lastMessagePreview?.takeIf { it.isNotBlank() }?.let {
                items.clear()
                items.add(TranscriptItem("preview", "assistant", it, session.lastMessageAt ?: now))
            }
            loading = false
        }
    }

    LaunchedEffect(session.chatId, client, indicator) {
        val c = client ?: return@LaunchedEffect
        while (true) {
            delay(if (indicator == ChatIndicator.working) 1_200 else 4_000)
            val loaded = runCatching { bridge.transcript(c, session) }.getOrNull() ?: continue
            if (loaded.isNotEmpty()) {
                items.clear()
                items.addAll(loaded)
            }
        }
    }

    LaunchedEffect(items.size) {
        if (items.isNotEmpty()) scrollState.animateScrollToItem(items.size - 1)
    }

    val working = indicator == ChatIndicator.working || sending

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(
                            session.title,
                            fontFamily = Geist,
                            fontWeight = FontWeight.SemiBold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            StatusCorner(indicator)
                            session.harness?.let {
                                Text(
                                    "  ·  ${harnessLabel(it)}",
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    fontSize = 12.sp,
                                    fontFamily = Geist,
                                    maxLines = 1,
                                )
                            }
                            session.model?.let {
                                Text(
                                    "  ·  $it",
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    fontSize = 12.sp,
                                    fontFamily = Geist,
                                    maxLines = 1,
                                )
                            }
                        }
                    }
                },
                navigationIcon = {
                    IconButton(onClick = onBack, modifier = Modifier.semantics { contentDescription = "Back" }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                ),
            )
        },
        containerColor = MaterialTheme.colorScheme.background,
    ) { insets ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(insets)
                .imePadding()
                .navigationBarsPadding(),
        ) {
            ConnectionBanner(link, onRetry = onReconnect)
            LazyColumn(
                state = scrollState,
                modifier = Modifier.weight(1f).fillMaxWidth(),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(
                    horizontal = 16.dp,
                    vertical = 18.dp,
                ),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                if (loading) {
                    item {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            CircularProgressIndicator(modifier = Modifier.size(16.dp), strokeWidth = 2.dp)
                            Spacer(Modifier.width(10.dp))
                            Text(
                                "Loading transcript…",
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                fontFamily = Geist,
                                fontSize = 13.sp,
                            )
                        }
                    }
                } else if (items.isEmpty()) {
                    item {
                        Column(Modifier.fillMaxWidth().padding(top = 48.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                            Text(
                                "No messages yet",
                                fontFamily = Geist,
                                fontWeight = FontWeight.SemiBold,
                                fontSize = 16.sp,
                            )
                            Spacer(Modifier.height(6.dp))
                            Text(
                                "When the desktop host is online, the transcript appears here.",
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                fontFamily = Geist,
                                fontSize = 14.sp,
                            )
                        }
                    }
                }
                items(items, key = { it.id }) { MessageBlock(it) }
                if (working) {
                    item { WorkingRow(session.chatId, session.statusUpdatedAt ?: now) }
                }
            }

            note?.let {
                Text(
                    it,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontFamily = Geist,
                    fontSize = 12.sp,
                    modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
                )
            }

            Row(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                session.model?.let {
                    session.harness?.let { h -> HarnessMark(h, size = 12.dp, neutral = cosmos.textMuted) }
                    Pill(it)
                }
                session.reasoning?.let { Pill(it) }
            }

            Composer(
                draft = draft,
                onDraft = { draft = it },
                working = working,
                enabled = client != null && link != LinkState.Offline,
                onSend = {
                    val text = draft.trim()
                    if (text.isEmpty()) return@Composer
                    items.add(
                        TranscriptItem("u-${System.currentTimeMillis()}", "user", text, System.currentTimeMillis()),
                    )
                    draft = ""
                    val c = client ?: return@Composer
                    sending = true
                    scope.launch {
                        when (val r = bridge.send(c, session, text)) {
                            is SendOutcome.Delivered -> note = null
                            is SendOutcome.Queued -> note = r.note
                            is SendOutcome.Failed -> note = r.message
                        }
                        sending = false
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
}

@Composable
private fun MessageBlock(item: TranscriptItem) {
    val cosmos = LocalCosmos.current
    if (item.role == "user") {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            Box(
                Modifier
                    .widthIn(max = 320.dp)
                    .clip(RoundedCornerShape(20.dp))
                    .background(cosmos.accent.copy(alpha = if (cosmos.isDark) 0.22f else 0.14f))
                    .padding(horizontal = 14.dp, vertical = 10.dp),
            ) {
                Text(item.text, color = cosmos.text, fontFamily = Geist, fontSize = 16.sp, lineHeight = 24.sp)
            }
        }
    } else {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Start) {
            Box(
                Modifier
                    .widthIn(max = 340.dp)
                    .clip(RoundedCornerShape(20.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = if (item.streaming) 0.7f else 1f))
                    .padding(horizontal = 14.dp, vertical = 10.dp),
            ) {
                Text(
                    item.text,
                    color = cosmos.text.copy(alpha = 0.92f),
                    fontFamily = Geist,
                    fontSize = 16.sp,
                    lineHeight = 24.sp,
                )
            }
        }
    }
}

@Composable
private fun WorkingRow(chatId: String, startedAt: Long) {
    val seed = remember(chatId) { Motion.flavourSeed(chatId) }
    val elapsed by produceState(0L) {
        while (true) {
            withInfiniteAnimationFrameMillis {
                value = (System.currentTimeMillis() - startedAt) / 1000
            }
        }
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        WorkingSpinner()
        Spacer(Modifier.width(10.dp))
        Text(
            Motion.flavourWord(seed, elapsed),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontFamily = Geist,
            fontWeight = FontWeight.Medium,
            fontSize = 14.sp,
        )
        Spacer(Modifier.width(8.dp))
        Text(
            Motion.formatElapsed(elapsed),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontFamily = Geist,
            fontSize = 12.sp,
        )
    }
}

@Composable
private fun Composer(
    draft: String,
    onDraft: (String) -> Unit,
    working: Boolean,
    enabled: Boolean,
    onSend: () -> Unit,
    onStop: () -> Unit,
) {
    val cosmos = LocalCosmos.current
    val hasContent = draft.isNotBlank()
    val stopMode = working && !hasContent
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.Bottom,
    ) {
        Row(
            Modifier
                .weight(1f)
                .clip(RoundedCornerShape(24.dp))
                .background(MaterialTheme.colorScheme.surfaceVariant)
                .padding(start = 16.dp, end = 8.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.weight(1f).padding(vertical = 8.dp)) {
                if (draft.isEmpty()) {
                    Text("Message", color = cosmos.textFaint, fontFamily = Geist, fontSize = 16.sp)
                }
                BasicTextField(
                    value = draft,
                    onValueChange = onDraft,
                    enabled = enabled,
                    textStyle = TextStyle(
                        color = cosmos.text,
                        fontFamily = Geist,
                        fontSize = 16.sp,
                        lineHeight = 22.sp,
                    ),
                    cursorBrush = SolidColor(cosmos.accent),
                    maxLines = 6,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(onSend = { if (hasContent && enabled) onSend() }),
                    modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Message composer" },
                )
            }
        }
        Spacer(Modifier.width(8.dp))
        val canAct = enabled && (hasContent || stopMode)
        Box(
            Modifier
                .size(48.dp)
                .clip(CircleShape)
                .background(if (canAct) cosmos.accentStrong else cosmos.elementActive)
                .then(if (canAct) Modifier.clickable { if (stopMode) onStop() else onSend() } else Modifier),
            contentAlignment = Alignment.Center,
        ) {
            if (stopMode) {
                Icon(
                    Icons.Filled.Stop,
                    contentDescription = "Stop",
                    tint = MaterialTheme.colorScheme.onPrimary,
                    modifier = Modifier.size(18.dp),
                )
            } else {
                Icon(
                    Icons.Filled.ArrowUpward,
                    contentDescription = "Send",
                    tint = if (hasContent && enabled) MaterialTheme.colorScheme.onPrimary else cosmos.textFaint,
                    modifier = Modifier.size(18.dp),
                )
            }
        }
    }
}
