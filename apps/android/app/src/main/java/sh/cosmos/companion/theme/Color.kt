package sh.cosmos.companion.theme

import androidx.compose.ui.graphics.Color

/**
 * Cosmos palette — computed from the exact oklch definitions in
 * crates/ui/src/theme.rs (the same OKLab matrices CSS Color 4 specifies), so
 * every surface and accent lands on the identical sRGB value the desktop and
 * iOS apps paint. Always-dark monochrome.
 */
object CosmosColors {
    val bg = Color(0xFF060606)              // main panel — #060606
    val surface = Color(0xFF0D0D0D)         // shell / sidebar — #0d0d0d
    val surfaceRaised = Color(0xFF1E1E1E)   // popovers, dialogs, cards
    val elementHover = Color(0x0FFFFFFF)    // white @ 0.06
    val elementActive = Color(0x1AFFFFFF)   // white @ 0.10
    val border = Color(0x14FFFFFF)          // white @ 0.08
    val borderStrong = Color(0x24FFFFFF)    // white @ 0.14

    val text = Color(0xFFE5E5E5)            // neutral-200
    val textMuted = Color(0xFFA1A1A1)       // neutral-400
    val textFaint = Color(0xFF737373)       // neutral-500

    val accent = Color(0xFF7C86FF)          // indigo-400
    val accentStrong = Color(0xFF615FFF)    // indigo-500
    val danger = Color(0xFFFF6467)          // red-400
    val dangerSoft = Color(0xFFFFA2A2)      // red-300
    val warning = Color(0xFFFFB900)         // amber-400

    val statusWorking = Color(0xFFFB64B6)   // pink-400
    val statusCompleted = Color(0xFF00D492) // emerald-400
    val statusAwaiting = Color(0xFFFFB900)  // amber (awaiting input)
    val statusIdle = Color(0xFF737373)      // neutral

    val inlineCodeText = Color(0xFFC4B4FF)  // violet-300
    val inlineCodeWash = Color(0x1F8B5CF6)  // violet-400 @ ~0.12

    val tokenKeyword = Color(0xFFE67E7F)
    val tokenString = Color(0xFF65CBA7)
    val tokenNumber = Color(0xFFE0AE57)

    val claudeBrand = Color(0xFFD97757)
}
