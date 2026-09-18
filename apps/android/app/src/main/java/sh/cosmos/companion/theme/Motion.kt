package sh.cosmos.companion.theme

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.Easing

/** Animation timings/curves ported from crates/ui/src/motion.rs (mirrored by
 *  apps/ios Motion.swift). */
object Motion {
    // Signature entrance: 500ms cubic-bezier(0.16, 1, 0.3, 1).
    val fadeIn: Easing = CubicBezierEasing(0.16f, 1f, 0.3f, 1f)
    val fadeQuick: Easing = CubicBezierEasing(0.25f, 0.1f, 0.25f, 1f)
    val resize: Easing = CubicBezierEasing(0f, 0f, 0.58f, 1f)
    val resort: Easing = CubicBezierEasing(0.22f, 1f, 0.36f, 1f)

    const val gradientSpinPeriodMs = 750L
    const val cosmosPulsePeriodMs = 2400L

    /** WorkingIndicator flavour words (transcript.rs), rotated every 7s, seeded per chat. */
    val flavourWords = listOf(
        "Cosmosing",
        "Thinking", "Pondering", "Scheming", "Brewing", "Weaving", "Tinkering",
        "Musing", "Composing", "Sifting", "Untangling", "Distilling", "Sketching",
        "Plotting", "Riffing", "Combobulating", "Percolating", "Marinating",
        "Noodling", "Puzzling", "Conjuring",
    )
    const val flavourRotateSecs = 7L

    /** FNV-1a, matching the desktop's per-chat seeding. */
    fun flavourSeed(chatId: String): Long {
        var hash = -0x340d631b7bdddcdbL // 0xcbf29ce484222325
        for (b in chatId.toByteArray()) {
            hash = hash xor (b.toLong() and 0xff)
            hash *= 0x100000001b3L
        }
        return hash
    }

    fun flavourWord(seed: Long, elapsedSecs: Long): String {
        val n = flavourWords.size.toLong()
        val ix = ((seed + (maxOf(0L, elapsedSecs) / flavourRotateSecs)) % n + n) % n
        return flavourWords[ix.toInt()]
    }

    fun formatElapsed(secs: Long): String {
        val s = maxOf(0L, secs)
        return if (s < 60) "${s}s" else "${s / 60}m ${s % 60}s"
    }
}
