package sh.cosmos.companion.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.cosmos.companion.net.ChatIndicator
import sh.cosmos.companion.theme.Geist
import sh.cosmos.companion.theme.LocalCosmos
import sh.cosmos.companion.vm.LinkState

@Composable
fun ChatIndicator.liveDot(): Color {
    val p = LocalCosmos.current
    return when (this) {
        ChatIndicator.working -> p.statusWorking.copy(alpha = 0.75f)
        ChatIndicator.awaitingInput -> p.accent.copy(alpha = 0.75f)
        ChatIndicator.errored -> p.danger.copy(alpha = 0.8f)
        ChatIndicator.completed -> p.statusCompleted
        ChatIndicator.idle -> p.textFaint.copy(alpha = 0.5f)
    }
}

fun ChatIndicator.statusLabel(): String? = when (this) {
    ChatIndicator.working -> "Working"
    ChatIndicator.awaitingInput -> "Input"
    ChatIndicator.errored -> "Failed"
    ChatIndicator.completed -> "Done"
    ChatIndicator.idle -> null
}

@Composable
fun StatusDot(indicator: ChatIndicator, modifier: Modifier = Modifier) {
    Box(
        modifier
            .size(8.dp)
            .clip(CircleShape)
            .background(indicator.liveDot())
            .semantics { contentDescription = indicator.statusLabel() ?: "Idle" },
    )
}

@Composable
fun StatusCorner(indicator: ChatIndicator) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        StatusDot(indicator)
        indicator.statusLabel()?.let {
            Spacer(Modifier.width(6.dp))
            Text(
                it,
                color = indicator.liveDot(),
                fontFamily = Geist,
                fontWeight = FontWeight.Medium,
                fontSize = 12.sp,
            )
        }
    }
}

@Composable
fun Pill(text: String, modifier: Modifier = Modifier, color: Color = LocalCosmos.current.textMuted) {
    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(50),
        color = LocalCosmos.current.elementHover,
        tonalElevation = 0.dp,
    ) {
        Text(
            text,
            color = color,
            fontFamily = Geist,
            fontSize = 12.sp,
            fontWeight = FontWeight.Medium,
            modifier = Modifier.padding(horizontal = 10.dp, vertical = 4.dp),
        )
    }
}

@Composable
fun ConnectionBanner(link: LinkState, onRetry: () -> Unit) {
    val p = LocalCosmos.current
    val (message, color) = when (link) {
        LinkState.Online -> return
        LinkState.Reconnecting -> "Reconnecting…" to p.warning
        LinkState.Offline -> "Offline — tap to retry" to p.danger
    }
    Surface(color = color.copy(alpha = 0.12f), modifier = Modifier.fillMaxWidth()) {
        Row(
            Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(message, color = color, fontFamily = Geist, fontSize = 13.sp, modifier = Modifier.weight(1f))
            if (link == LinkState.Offline) {
                TextButton(onClick = onRetry) { Text("Retry", color = color) }
            }
        }
    }
}

fun relativeTime(ms: Long): String {
    val delta = maxOf(0L, System.currentTimeMillis() - ms) / 1000
    return when {
        delta < 60 -> "now"
        delta < 3600 -> "${delta / 60}m"
        delta < 86_400 -> "${delta / 3600}h"
        else -> "${delta / 86_400}d"
    }
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
    "kimi" -> "Kimi"
    "mock" -> "Mock"
    else -> id.replaceFirstChar { it.uppercase() }
}
