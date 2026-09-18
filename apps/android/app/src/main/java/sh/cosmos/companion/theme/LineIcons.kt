package sh.cosmos.companion.theme

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * Stroked UI glyphs — the desktop's Solar-Linear-style icons
 * (crates/ui/assets/icons), same path data, stroked at 1.5/24 with round caps.
 * Mirrors apps/ios LineIcons.swift.
 */
@Composable
fun GitBranchIcon(size: Dp = 12.dp, color: Color = CosmosColors.textMuted) {
    val path = remember {
        Path().apply {
            addPath(PathParser().parsePathString("M6.5 7.75v8.5").toPath())
            addPath(PathParser().parsePathString(
                "M17.5 9.75c0 2.9-2.6 4.35-6.2 4.72c-1.9.2-3.3.9-4 2.03").toPath())
            // circles: (cx, cy, r)
            listOf(Triple(6.5f, 5.5f, 2.25f), Triple(6.5f, 18.5f, 2.25f), Triple(17.5f, 7.5f, 2.25f))
                .forEach { (cx, cy, r) -> addOval(Rect(cx - r, cy - r, cx + r, cy + r)) }
        }
    }
    Canvas(Modifier.size(size)) {
        val s = minOf(this.size.width, this.size.height) / 24f
        val dx = (this.size.width - 24f * s) / 2f
        val dy = (this.size.height - 24f * s) / 2f
        withTransform({
            translate(dx, dy)
            scale(s, s, pivot = Offset.Zero)
        }) {
            drawPath(path, color = color, style = Stroke(width = 1.5f, cap = StrokeCap.Round, join = StrokeJoin.Round))
        }
    }
}
