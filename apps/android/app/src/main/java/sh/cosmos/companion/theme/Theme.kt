package sh.cosmos.companion.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable

private val CosmosScheme = darkColorScheme(
    primary = CosmosColors.accent,
    onPrimary = CosmosColors.text,
    secondary = CosmosColors.accentStrong,
    background = CosmosColors.bg,
    onBackground = CosmosColors.text,
    surface = CosmosColors.surface,
    onSurface = CosmosColors.text,
    surfaceVariant = CosmosColors.surfaceRaised,
    onSurfaceVariant = CosmosColors.textMuted,
    error = CosmosColors.danger,
    outline = CosmosColors.border,
)

/** Cosmos is always dark; the system dark flag is ignored on purpose. */
@Composable
fun CosmosTheme(darkTheme: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = CosmosScheme,
        typography = CosmosTypography,
        content = content,
    )
}
