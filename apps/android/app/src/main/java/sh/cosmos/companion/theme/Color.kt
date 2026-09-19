package sh.cosmos.companion.theme

import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

/**
 * Cosmos palettes for phone. Dark is the product default (same oklch-derived
 * sRGB as desktop/iOS). Light is a first-class Material scheme, not an
 * inverted screenshot of the desktop — paper surfaces, indigo accent, Geist.
 */
data class CosmosPalette(
    val bg: Color,
    val surface: Color,
    val surfaceRaised: Color,
    val elementHover: Color,
    val elementActive: Color,
    val border: Color,
    val borderStrong: Color,
    val text: Color,
    val textMuted: Color,
    val textFaint: Color,
    val accent: Color,
    val accentStrong: Color,
    val danger: Color,
    val dangerSoft: Color,
    val warning: Color,
    val statusWorking: Color,
    val statusCompleted: Color,
    val statusAwaiting: Color,
    val statusIdle: Color,
    val inlineCodeText: Color,
    val inlineCodeWash: Color,
    val isDark: Boolean,
) {
    companion object {
        val Dark = CosmosPalette(
            bg = Color(0xFF060606),
            surface = Color(0xFF0D0D0D),
            surfaceRaised = Color(0xFF1E1E1E),
            elementHover = Color(0x0FFFFFFF),
            elementActive = Color(0x1AFFFFFF),
            border = Color(0x14FFFFFF),
            borderStrong = Color(0x24FFFFFF),
            text = Color(0xFFE5E5E5),
            textMuted = Color(0xFFA1A1A1),
            textFaint = Color(0xFF737373),
            accent = Color(0xFF7C86FF),
            accentStrong = Color(0xFF615FFF),
            danger = Color(0xFFFF6467),
            dangerSoft = Color(0xFFFFA2A2),
            warning = Color(0xFFFFB900),
            statusWorking = Color(0xFFFB64B6),
            statusCompleted = Color(0xFF00D492),
            statusAwaiting = Color(0xFFFFB900),
            statusIdle = Color(0xFF737373),
            inlineCodeText = Color(0xFFC4B4FF),
            inlineCodeWash = Color(0x1F8B5CF6),
            isDark = true,
        )
        val Light = CosmosPalette(
            bg = Color(0xFFF4F4F2),
            surface = Color(0xFFFFFFFF),
            surfaceRaised = Color(0xFFEEEEEC),
            elementHover = Color(0x14000000),
            elementActive = Color(0x1F000000),
            border = Color(0x1A000000),
            borderStrong = Color(0x33000000),
            text = Color(0xFF171717),
            textMuted = Color(0xFF525252),
            textFaint = Color(0xFF737373),
            accent = Color(0xFF4F46E5),
            accentStrong = Color(0xFF4338CA),
            danger = Color(0xFFDC2626),
            dangerSoft = Color(0xFFEF4444),
            warning = Color(0xFFD97706),
            statusWorking = Color(0xFFDB2777),
            statusCompleted = Color(0xFF059669),
            statusAwaiting = Color(0xFFD97706),
            statusIdle = Color(0xFF737373),
            inlineCodeText = Color(0xFF6D28D9),
            inlineCodeWash = Color(0x1A7C3AED),
            isDark = false,
        )
    }
}

val LocalCosmos = staticCompositionLocalOf { CosmosPalette.Dark }

/** Always-dark object kept for call sites that do not sit under [LocalCosmos]. */
object CosmosColors {
    val bg get() = CosmosPalette.Dark.bg
    val surface get() = CosmosPalette.Dark.surface
    val surfaceRaised get() = CosmosPalette.Dark.surfaceRaised
    val elementHover get() = CosmosPalette.Dark.elementHover
    val elementActive get() = CosmosPalette.Dark.elementActive
    val border get() = CosmosPalette.Dark.border
    val borderStrong get() = CosmosPalette.Dark.borderStrong
    val text get() = CosmosPalette.Dark.text
    val textMuted get() = CosmosPalette.Dark.textMuted
    val textFaint get() = CosmosPalette.Dark.textFaint
    val accent get() = CosmosPalette.Dark.accent
    val accentStrong get() = CosmosPalette.Dark.accentStrong
    val danger get() = CosmosPalette.Dark.danger
    val dangerSoft get() = CosmosPalette.Dark.dangerSoft
    val warning get() = CosmosPalette.Dark.warning
    val statusWorking get() = CosmosPalette.Dark.statusWorking
    val statusCompleted get() = CosmosPalette.Dark.statusCompleted
    val statusAwaiting get() = CosmosPalette.Dark.statusAwaiting
    val statusIdle get() = CosmosPalette.Dark.statusIdle
    val inlineCodeText get() = CosmosPalette.Dark.inlineCodeText
    val inlineCodeWash get() = CosmosPalette.Dark.inlineCodeWash
    val claudeBrand = Color(0xFFD97757)
}

enum class ThemeMode { System, Dark, Light }
