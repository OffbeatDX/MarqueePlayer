package com.offbeat.marquee.player.ui.components

import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.window.DialogWindowProvider
import androidx.core.view.WindowCompat

/**
 * Call first thing inside a ModalBottomSheet's content. Material 3 (1.3) gives the sheet's own
 * window light or dark system-bar icons from the *system's* dark mode, not the app's theme, which
 * is always dark: on a phone in light mode the sheet got dark status icons over the dimmed screen,
 * and with three-button navigation a light grey scrim under its buttons. This keeps the bars as
 * the rest of the app has them: light icons, and the sheet's surface drawn under the buttons.
 */
@Composable
fun DarkSheetBars() {
    val window = (LocalView.current.parent as? DialogWindowProvider)?.window ?: return
    val view = LocalView.current
    SideEffect {
        WindowCompat.getInsetsController(window, view).apply {
            isAppearanceLightStatusBars = false
            isAppearanceLightNavigationBars = false
        }
        window.isNavigationBarContrastEnforced = false
    }
}
