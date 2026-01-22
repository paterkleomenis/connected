package com.connected.app

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

// Colors from desktop styles.css
val ColorBgPrimary = Color(0xFF000000)
val ColorBgSecondary = Color(0xFF1c1c1e)
val ColorBgTertiary = Color(0xFF2c2c2e)
val ColorAccent = Color(0xFF0a84ff)
val ColorTextPrimary = Color(0xFFf5f5f7)
val ColorTextSecondary = Color(0xFFa1a1a6)
val ColorSuccess = Color(0xFF30d158)
val ColorError = Color(0xFFff453a)

private val ConnectedDarkColorScheme = darkColorScheme(
    primary = ColorAccent,
    onPrimary = Color.White,
    primaryContainer = Color(0xFF003D7A), // Darker shade of accent
    onPrimaryContainer = ColorTextPrimary,

    secondary = ColorBgSecondary,
    onSecondary = ColorTextPrimary,

    tertiary = ColorSuccess,

    background = ColorBgPrimary,
    onBackground = ColorTextPrimary,

    surface = ColorBgSecondary,
    onSurface = ColorTextPrimary,

    surfaceVariant = ColorBgTertiary,
    onSurfaceVariant = ColorTextSecondary,

    error = ColorError,
    onError = Color.White
)

// We primarily support Dark Theme to match desktop default, but providing a light mapping if needed
// For now, mapping light theme to be similar or just standard light
private val ConnectedLightColorScheme = lightColorScheme(
    primary = ColorAccent,
    onPrimary = Color.White,
    background = Color(0xFFf5f5f7),
    onBackground = Color(0xFF1d1d1f),
    surface = Color(0xFFffffff),
    onSurface = Color(0xFF1d1d1f),
    surfaceVariant = Color(0xFFe8e8ed),
    onSurfaceVariant = Color(0xFF6e6e73)
)

@Composable
fun ConnectedTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit
) {
    val colorScheme = when {
        // Force Dark Theme preference if user is in dark mode or if we want to enforce it
        // The desktop app defaults to dark, so we prioritize it.
        darkTheme -> ConnectedDarkColorScheme
        else -> ConnectedLightColorScheme
    }

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
        typography = MaterialTheme.typography,
        content = content
    )
}
