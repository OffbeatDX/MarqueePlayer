package com.offbeat.marquee.player.ui.player

import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.horizontalDrag
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import com.offbeat.marquee.player.playback.NowPlaying
import com.offbeat.marquee.player.ui.components.formatTime
import com.offbeat.marquee.player.ui.theme.Palette
import androidx.compose.foundation.layout.Box

/**
 * Now Playing's seek bar: a thin track (4 dp), the elapsed part in the accent, a small round thumb
 * (12 dp) that grows while held, in a 48 dp high touch area. Dragging (or a tap) previews the time
 * in the labels and seeks once, on release. The bar moves from the position snapshot in the draw
 * phase only; the labels recompose once a second.
 *
 * A seek waiting for the track to load ([NowPlaying.seekPending]: a live transcode seeks once it is
 * whole on the phone) holds the bar at its position and says so in place of the time left.
 */
@Composable
fun SeekBar(np: NowPlaying, onSeek: (Long) -> Unit, modifier: Modifier = Modifier) {
    val palette = Palette.current
    var barPx by remember { mutableIntStateOf(0) }
    val clock = rememberPlaybackClock(np, stepFor(np.durationMs, barPx))
    var dragging by remember { mutableStateOf(false) }
    var dragFraction by remember { mutableFloatStateOf(0f) }
    val duration = np.durationMs.coerceAtLeast(1)
    val current by rememberUpdatedState(np)
    val seek by rememberUpdatedState(onSeek)
    val thumb by animateDpAsState(if (dragging) 20.dp else 12.dp, tween(120), label = "thumb")
    val playedFraction = { (current.positionAt(clock.value).toFloat() / duration).coerceIn(0f, 1f) }
    // The labels follow whole seconds only.
    val shownSeconds by remember(np, duration) {
        derivedStateOf { (if (dragging) (dragFraction * duration).toLong() else current.positionAt(clock.value)) / 1000 }
    }
    val shown = shownSeconds * 1000
    Column(modifier) {
        Box(
            Modifier
                .fillMaxWidth()
                .height(48.dp)
                .onSizeChanged { barPx = it.width }
                .pointerInput(duration) {
                    awaitEachGesture {
                        val down = awaitFirstDown()
                        down.consume()
                        val w = size.width.toFloat().coerceAtLeast(1f)
                        dragging = true
                        dragFraction = (down.position.x / w).coerceIn(0f, 1f)
                        val completed = horizontalDrag(down.id) { change ->
                            dragFraction = (change.position.x / w).coerceIn(0f, 1f)
                            change.consume()
                        }
                        // A release (drag or tap) seeks; a cancelled gesture leaves the position alone.
                        if (completed) seek((dragFraction * current.durationMs).toLong())
                        dragging = false
                    }
                }
                .semantics {
                    stateDescription = "${formatTime(shown)} of ${formatTime(np.durationMs)}" +
                        if (np.seekPending) ", seeking once the track has loaded" else ""
                    progressBarRangeInfo = ProgressBarRangeInfo(shown.toFloat() / duration, 0f..1f)
                    setProgress { f -> seek((f.coerceIn(0f, 1f) * current.durationMs).toLong()); true }
                }
                .drawBehind {
                    val track = 4.dp.toPx()
                    val y = size.height / 2
                    val f = if (dragging) dragFraction else playedFraction()
                    val r = CornerRadius(track / 2, track / 2)
                    drawRoundRect(palette.text.copy(alpha = 0.18f), Offset(0f, y - track / 2), Size(size.width, track), r)
                    drawRoundRect(palette.accent, Offset(0f, y - track / 2), Size(size.width * f, track), r)
                    val radius = thumb.toPx() / 2
                    drawCircle(palette.accent, radius, Offset((size.width * f).coerceIn(radius, size.width - radius), y))
                },
        )
        Row(Modifier.fillMaxWidth()) {
            Text(formatTime(shown), style = MaterialTheme.typography.bodySmall, color = if (dragging) palette.accent else palette.textDim)
            Spacer(Modifier.weight(1f))
            if (np.seekPending && !dragging) {
                Text("Seeking once loaded", style = MaterialTheme.typography.bodySmall, color = palette.accent)
            } else {
                Text("-" + formatTime((np.durationMs - shown).coerceAtLeast(0)), style = MaterialTheme.typography.bodySmall, color = palette.textDim)
            }
        }
    }
}
