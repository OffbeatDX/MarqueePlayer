package com.offbeat.marquee.player.ui.player

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.playback.PlaybackCommands
import com.offbeat.marquee.player.playback.QueueRow
import com.offbeat.marquee.player.playback.SleepState
import com.offbeat.marquee.player.ui.components.formatTime
import com.offbeat.marquee.player.ui.components.DarkSheetBars
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.coroutines.delay

/** The sleep timer: off, 15 / 30 / 45 / 60 minutes, or the end of this track; shows the time left. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SleepSheet(sleep: SleepState, onChoose: (Int) -> Unit, onDismiss: () -> Unit) {
    val palette = Palette.current
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = palette.surface,
    ) {
        DarkSheetBars()
        Column(Modifier.padding(bottom = 16.dp)) {
            Text("Sleep timer", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(horizontal = 20.dp).semantics { heading() })
            Text(
                sleepLabel(sleep, short = false) ?: "Off",
                style = MaterialTheme.typography.bodyMedium,
                color = if (sleep == SleepState.Off) palette.textDim else palette.accent,
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
            )
            val options = listOf(
                "Off" to PlaybackCommands.SLEEP_OFF,
                "15 minutes" to 15,
                "30 minutes" to 30,
                "45 minutes" to 45,
                "1 hour" to 60,
                "End of this track" to PlaybackCommands.SLEEP_END_OF_TRACK,
            )
            for ((label, minutes) in options) {
                val chosen = (minutes == PlaybackCommands.SLEEP_OFF && sleep == SleepState.Off) ||
                    (minutes == PlaybackCommands.SLEEP_END_OF_TRACK && sleep == SleepState.EndOfTrack)
                Row(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(min = 56.dp)
                        .clickable(role = Role.Button) { onChoose(minutes) }
                        .padding(horizontal = 20.dp)
                        .semantics { selected = chosen },
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(label, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
                    Box(Modifier.size(24.dp)) {
                        if (chosen) Icon(Icons.Rounded.Check, contentDescription = null, tint = palette.accent)
                    }
                }
            }
        }
    }
}

/**
 * "23 min" (short) or "Stops in 23 min" for a set timer, ticking every 15 s while drawn; null when off.
 */
@Composable
fun sleepLabel(sleep: SleepState, short: Boolean): String? {
    val now = remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(sleep) {
        if (sleep !is SleepState.At) return@LaunchedEffect
        while (true) {
            withFrameMillis { } // nothing ticks while the screen isn't drawn
            now.longValue = System.currentTimeMillis()
            delay(15_000)
        }
    }
    return when (sleep) {
        SleepState.Off -> null
        SleepState.EndOfTrack -> if (short) "Track" else "Stops at the end of this track"
        is SleepState.At -> {
            val minutes = ((sleep.endsAtWallMs - now.longValue + 59_999) / 60_000).coerceAtLeast(0)
            if (short) "$minutes min" else "Stops in $minutes min"
        }
    }
}
