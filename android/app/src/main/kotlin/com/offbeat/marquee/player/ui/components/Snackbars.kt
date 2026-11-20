package com.offbeat.marquee.player.ui.components

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Snackbar
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens

/**
 * The app's snackbar host: Material's snackbar in the dark theme (the raised surface, the text
 * colour, the action in the accent) instead of its light default. The theme's inverse colours say
 * the same, for any snackbar or tooltip that doesn't come through here.
 */
@Composable
fun ThemedSnackbarHost(state: SnackbarHostState, modifier: Modifier = Modifier) {
    val palette = Palette.current
    SnackbarHost(state, modifier) { data ->
        Snackbar(
            data,
            shape = RoundedCornerShape(Tokens.RadiusCard),
            containerColor = palette.surfaceHi,
            contentColor = palette.text,
            actionColor = palette.accent,
            actionContentColor = palette.accent,
            dismissActionContentColor = palette.textDim,
        )
    }
}
