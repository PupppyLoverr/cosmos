package sh.cosmos.companion.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.cosmos.companion.net.ChatIndicator
import sh.cosmos.companion.theme.CosmosColors
import sh.cosmos.companion.theme.Geist

/** shell/spaces.rs status_dot_color — non-done states are muted (running is
 *  routine); only Done keeps its pop. Mirrors apps/ios ChatIndicator.dotColor. */
fun ChatIndicator.dotColor(): Color = when (this) {
    ChatIndicator.working -> CosmosColors.statusWorking.copy(alpha = 0.55f)
    ChatIndicator.awaitingInput -> CosmosColors.accent.copy(alpha = 0.6f)
    ChatIndicator.errored -> CosmosColors.danger.copy(alpha = 0.65f)
    ChatIndicator.completed -> CosmosColors.statusCompleted.copy(alpha = 0.9f)
    ChatIndicator.idle -> Color.White.copy(alpha = 0.14f)
}

/** shell.rs status word; null (idle) renders the time-ago instead. */
fun ChatIndicator.statusLabel(): String? = when (this) {
    ChatIndicator.working -> "Working"
    ChatIndicator.awaitingInput -> "Input"
    ChatIndicator.errored -> "Failed"
    ChatIndicator.completed -> "Done"
    ChatIndicator.idle -> null
}

@Composable
fun StatusDot(indicator: ChatIndicator, modifier: Modifier = Modifier) {
    Box(modifier.size(6.dp).clip(CircleShape).background(indicator.dotColor()))
}

/** The row's top-right status glyph: a 6pt dot + word in the same muted color;
 *  Done trades the dot for a check. */
@Composable
fun StatusCorner(indicator: ChatIndicator) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        if (indicator == ChatIndicator.completed) {
            Icon(Icons.Filled.Check, null, tint = indicator.dotColor(), modifier = Modifier.size(11.dp))
        } else {
            StatusDot(indicator)
        }
        indicator.statusLabel()?.let {
            Spacer(Modifier.width(4.dp))
            Text(it, color = indicator.dotColor(), fontFamily = Geist, fontWeight = FontWeight.Medium, fontSize = 10.sp)
        }
    }
}

/** A restrained hairline pill — used for meta chips (harness/model/branch). */
@Composable
fun Pill(text: String, modifier: Modifier = Modifier, color: Color = CosmosColors.textMuted) {
    Box(
        modifier
            .clip(RoundedCornerShape(6.dp))
            .background(CosmosColors.elementHover)
            .border(1.dp, CosmosColors.border, RoundedCornerShape(6.dp))
            .padding(horizontal = 8.dp, vertical = 3.dp),
    ) {
        Text(text, color = color, fontFamily = Geist, fontSize = 12.sp, fontWeight = FontWeight.Medium)
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
    "mock" -> "Mock"
    else -> id.replaceFirstChar { it.uppercase() }
}
