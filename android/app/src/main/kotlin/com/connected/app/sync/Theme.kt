package com.connected.app.sync

import android.app.Activity
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat

enum class ThemeMode(val storageValue: String) {
    SYSTEM("system"),
    LIGHT("light"),
    DARK("dark");

    companion object {
        fun fromStorageValue(value: String?): ThemeMode {
            return entries.firstOrNull { it.storageValue == value } ?: SYSTEM
        }
    }
}

private val ColorBgPrimary = Color(0xFF000000)
private val ColorBgSecondary = Color(0xFF000000)
private val ColorBgTertiary = Color(0xFF1C1C1E)
private val ColorAccent = Color(0xFFFFFFFF)
private val ColorLightText = Color(0xFF1D1D1F)
private val ColorLightBackground = Color(0xFFF5F5F7)
private val ColorLightSurface = Color(0xFFFFFFFF)
private val ColorLightSurfaceVariant = Color(0xFFE8E8ED)
private val ColorSuccess = Color(0xFF30D158)
private val ColorError = Color(0xFFFF453A)

private val ConnectedDarkColorScheme = darkColorScheme(
    primary = ColorAccent,
    onPrimary = Color.Black,
    primaryContainer = Color(0xFF333333),
    onPrimaryContainer = Color.White,

    secondary = ColorBgSecondary,
    onSecondary = Color.White,
    secondaryContainer = Color(0xFF333333),
    onSecondaryContainer = Color.White,

    tertiary = ColorSuccess,
    onTertiary = Color.Black,
    tertiaryContainer = Color(0xFF1E5C35),
    onTertiaryContainer = Color(0xFFB8F7CC),

    background = ColorBgPrimary,
    onBackground = Color.White,

    surface = ColorBgSecondary,
    onSurface = Color.White,

    surfaceVariant = ColorBgTertiary,
    onSurfaceVariant = Color(0xFFD1D1D6),
    outline = Color(0xFF8E8E93),
    outlineVariant = Color(0xFF3A3A3C),

    error = ColorError,
    onError = Color.White,
    errorContainer = Color(0xFF5A1A16),
    onErrorContainer = Color(0xFFFFDAD6)
)

private val ConnectedLightColorScheme = lightColorScheme(
    primary = Color.Black,
    onPrimary = Color.White,
    primaryContainer = ColorLightSurfaceVariant,
    onPrimaryContainer = ColorLightText,

    secondary = ColorLightText,
    onSecondary = Color.White,
    secondaryContainer = Color(0xFFD9D9DE),
    onSecondaryContainer = ColorLightText,

    tertiary = ColorSuccess,
    onTertiary = Color.Black,
    tertiaryContainer = Color(0xFFB8F7CC),
    onTertiaryContainer = Color(0xFF00210F),

    background = ColorLightBackground,
    onBackground = ColorLightText,

    surface = ColorLightSurface,
    onSurface = ColorLightText,

    surfaceVariant = ColorLightSurfaceVariant,
    onSurfaceVariant = Color(0xFF3D3D42),
    outline = Color(0xFF737378),
    outlineVariant = Color(0xFFC8C8CC),

    error = ColorError,
    onError = Color.White,
    errorContainer = Color(0xFFF9DEDC),
    onErrorContainer = Color(0xFF410E0B)
)

@Composable
fun ConnectedTheme(
    themeMode: ThemeMode = ThemeMode.SYSTEM,
    content: @Composable () -> Unit
) {
    val systemDarkTheme = isSystemInDarkTheme()
    val darkTheme = when (themeMode) {
        ThemeMode.SYSTEM -> systemDarkTheme
        ThemeMode.LIGHT -> false
        ThemeMode.DARK -> true
    }

    val colorScheme = if (darkTheme) ConnectedDarkColorScheme else ConnectedLightColorScheme

    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = (view.context as Activity).window
            WindowCompat.getInsetsController(window, view).isAppearanceLightStatusBars = !darkTheme
            WindowCompat.getInsetsController(window, view).isAppearanceLightNavigationBars = !darkTheme
        }
    }

    MaterialTheme(
        colorScheme = colorScheme,
        content = content
    )
}
