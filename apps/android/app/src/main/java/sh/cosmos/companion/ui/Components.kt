package sh.cosmos.companion.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.cosmos.companion.net.ChatIndicator
import sh.cosmos.companion.theme.CosmosColors

fun ChatIndicator.color(): Color = when (this) {
    ChatIndicator.awaitingInput -> CosmosColors.warning
    ChatIndicator.errored -> CosmosColors.danger
    ChatIndicator.working -> CosmosColors.statusWorking
    ChatIndicator.completed -> CosmosColors.statusCompleted
    ChatIndicator.idle -> CosmosColors.textFaint
}

fun ChatIndicator.label(): String = when (this) {
    ChatIndicator.awaitingInput -> "Awaiting input"
    ChatIndicator.errored -> "Error"
    ChatIndicator.working -> "Working"
    ChatIndicator.completed -> "Done"
    ChatIndicator.idle -> "Idle"
}

@Composable
fun StatusDot(indicator: ChatIndicator, modifier: Modifier = Modifier) {
    Box(
        modifier
            .size(8.dp)
            .clip(CircleShape)
            .background(indicator.color()),
    )
}

/** A restrained hairline pill — the mono-surface chip used for harness/model. */
@Composable
fun Pill(text: String, modifier: Modifier = Modifier, color: Color = CosmosColors.textMuted) {
    Box(
        modifier
            .clip(RoundedCornerShape(6.dp))
            .background(CosmosColors.elementHover)
            .border(1.dp, CosmosColors.border, RoundedCornerShape(6.dp))
            .padding(horizontal = 8.dp, vertical = 3.dp),
    ) {
        Text(text, color = color, fontSize = 12.sp, fontWeight = FontWeight.Medium)
    }
}

@Composable
fun DotLabel(indicator: ChatIndicator) {
    Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
        StatusDot(indicator)
        Text(
            "  ${indicator.label()}",
            color = if (indicator == ChatIndicator.idle) CosmosColors.textFaint else CosmosColors.textMuted,
            fontSize = 12.sp,
        )
    }
}
