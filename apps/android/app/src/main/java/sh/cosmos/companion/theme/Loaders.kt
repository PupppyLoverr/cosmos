package sh.cosmos.companion.theme

import androidx.compose.animation.core.withInfiniteAnimationFrameMillis
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlin.math.cos
import kotlin.math.sqrt

/**
 * Loaders + status indicators — ports of crates/ui/src/loaders.rs (mirrored by
 * apps/ios Loaders.swift). The 3×3/2×3 gradient-spin grids pulse per-cell with
 * a phase offset so a light wave travels the grid.
 */
private object GradientSpin {
    // GSPIN_ROW_TINTS: row0 cool blue, row1 amber, row2 pink.
    val rowTints = listOf(
        Color(0xFFB6D3EF),
        Color(0xFFEDB185),
        Color(0xFFF888A0),
    )
    const val dim = 0.1

    fun opacity(phaseIn: Double): Double {
        var p = phaseIn % 1.0
        if (p < 0) p += 1.0
        return when {
            p < 0.45 -> {
                val t = p / 0.45
                1 - (1 - dim) * (t * t * (3 - 2 * t))
            }
            p < 0.92 -> dim
            else -> {
                val t = (p - 0.92) / 0.08
                dim + (1 - dim) * t
            }
        }
    }
}

@Composable
private fun animationSeconds(): State<Double> =
    produceState(0.0) {
        while (true) {
            withInfiniteAnimationFrameMillis { value = it / 1000.0 }
        }
    }

/** 2×3 mini spinner — cells snake clockwise around the perimeter ring.
 *  Marks Working rows in the session list. */
@Composable
fun MiniSpinner(cell: Dp = 2.dp) {
    val time by animationSeconds()
    val ring = listOf(0 to 0, 0 to 1, 1 to 1, 2 to 1, 2 to 0, 1 to 0)
    Canvas(Modifier.size(cell * 4)) {
        val c = cell.toPx()
        val gap = c * 0.8f
        val t = time / (Motion.gradientSpinPeriodMs / 1000.0)
        for (row in 0..2) for (col in 0..1) {
            val ix = ring.indexOf(row to col).coerceAtLeast(0)
            val phase = ix.toDouble() / ring.size
            val op = GradientSpin.opacity(t - phase).toFloat()
            drawRect(
                color = GradientSpin.rowTints[row].copy(alpha = op),
                topLeft = Offset(col * (c + gap), row * (c + gap)),
                size = Size(c, c),
            )
        }
    }
}

/** 3×3 working indicator (arrow-up wave from bottom-center). */
@Composable
fun WorkingSpinner(cell: Dp = 3.dp) {
    val time by animationSeconds()
    Canvas(Modifier.size(cell * 5)) {
        val c = cell.toPx()
        val gap = c * 0.8f
        val t = time / (Motion.gradientSpinPeriodMs / 1000.0)
        for (row in 0..2) for (col in 0..2) {
            val dx = (col - 1).toDouble()
            val dy = (2 - row).toDouble()
            val dist = sqrt(dx * dx + dy * dy) / 2.5
            val op = GradientSpin.opacity(t - dist).toFloat()
            drawRect(
                color = GradientSpin.rowTints[row].copy(alpha = op),
                topLeft = Offset(col * (c + gap), row * (c + gap)),
                size = Size(c, c),
            )
        }
    }
}

/** cosmos-pulse loading row: 5 cells, cosine wave. */
@Composable
fun CosmosPulse(cell: Dp = 6.dp, color: Color = CosmosColors.text) {
    val time by animationSeconds()
    Canvas(Modifier.size(width = cell * 7, height = cell)) {
        val c = cell.toPx()
        val gap = c / 2f
        for (ix in 0..4) {
            var phase = (time / (Motion.cosmosPulsePeriodMs / 1000.0) - ix * (0.15 / 2.4)) % 1.0
            if (phase < 0) phase += 1.0
            val wave = (1 - cos(phase * 2 * Math.PI)) / 2
            val op = (0.08 + 0.92 * wave).toFloat()
            val scale = (0.9 + 0.1 * wave).toFloat()
            val inset = c * (1 - scale) / 2f
            drawRect(
                color = color.copy(alpha = op),
                topLeft = Offset(ix * (c + gap) + inset, inset),
                size = Size(c * scale, c * scale),
            )
        }
    }
}
