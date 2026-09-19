package sh.cosmos.companion.theme

import android.app.Activity
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat

private fun scheme(p: CosmosPalette) = if (p.isDark) {
    darkColorScheme(
        primary = p.accent,
        onPrimary = Color.White,
        secondary = p.accentStrong,
        onSecondary = Color.White,
        background = p.bg,
        onBackground = p.text,
        surface = p.surface,
        onSurface = p.text,
        surfaceVariant = p.surfaceRaised,
        onSurfaceVariant = p.textMuted,
        error = p.danger,
        onError = Color.White,
        outline = p.borderStrong,
        outlineVariant = p.border,
    )
} else {
    lightColorScheme(
        primary = p.accent,
        onPrimary = Color.White,
        secondary = p.accentStrong,
        onSecondary = Color.White,
        background = p.bg,
        onBackground = p.text,
        surface = p.surface,
        onSurface = p.text,
        surfaceVariant = p.surfaceRaised,
        onSurfaceVariant = p.textMuted,
        error = p.danger,
        onError = Color.White,
        outline = p.borderStrong,
        outlineVariant = p.border,
    )
}

@Composable
fun CosmosTheme(
    mode: ThemeMode = ThemeMode.System,
    content: @Composable () -> Unit,
) {
    val systemDark = isSystemInDarkTheme()
    val dark = when (mode) {
        ThemeMode.System -> systemDark
        ThemeMode.Dark -> true
        ThemeMode.Light -> false
    }
    val palette = if (dark) CosmosPalette.Dark else CosmosPalette.Light
    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = (view.context as? Activity)?.window ?: return@SideEffect
            WindowCompat.getInsetsController(window, view).apply {
                isAppearanceLightStatusBars = !dark
                isAppearanceLightNavigationBars = !dark
            }
        }
    }
    CompositionLocalProvider(LocalCosmos provides palette) {
        MaterialTheme(
            colorScheme = scheme(palette),
            typography = CosmosTypography,
            content = content,
        )
    }
}
