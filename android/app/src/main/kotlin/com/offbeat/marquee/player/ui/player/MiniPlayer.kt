package com.offbeat.marquee.player.ui.player

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.SkipNext
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.playback.NowPlaying
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.coroutines.launch
import kotlin.math.abs

/**
 * The mini player: cover, title, artist, play/pause and next, a progress hairline along its top
 * edge. Swipe sideways for previous / next, tap or swipe up for Now Playing. [active] false (Now
 * Playing covers it) stops the hairline's clock, so a covered hairline is not redrawn.
 */
@Composable
fun MiniPlayer(
    np: NowPlaying,
    onTogglePlay: () -> Unit,
    onPrevious: () -> Unit,
    onNext: () -> Unit,
    onOpen: () -> Unit,
    modifier: Modifier = Modifier,
    active: Boolean = true,
) {
    val palette = Palette.current
    val scope = rememberCoroutineScope()
    val dragX = remember { Animatable(0f) }
    var width by remember { mutableIntStateOf(0) }
    val clock = rememberPlaybackClock(np, stepFor(np.durationMs, width), enabled = active)
    val threshold = 0.22f
    Box(
        modifier
            .padding(horizontal = 8.dp, vertical = 6.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(palette.surfaceHi)
            .onSizeChanged { width = it.width }
            // The hairline: drawn from the clock in the draw phase, so a step only redraws it.
            .drawWithContent {
                drawContent()
                val h = 2.dp.toPx()
                drawRect(palette.text.copy(alpha = 0.12f), size = Size(size.width, h))
                val d = np.durationMs
                if (d > 0) {
                    val f = np.positionAt(clock.value).toFloat() / d
                    drawRect(palette.accent, topLeft = Offset.Zero, size = Size(size.width * f.coerceIn(0f, 1f), h))
                }
            }
            .pointerInput(np.hasNext) {
                detectHorizontalDragGestures(
                    onDragEnd = {
                        val w = size.width.toFloat()
                        val x = dragX.value
                        scope.launch {
                            when {
                                x < -w * threshold && np.hasNext -> { dragX.animateTo(-w, tween(120)); onNext(); dragX.snapTo(0f) }
                                x > w * threshold -> { dragX.animateTo(w, tween(120)); onPrevious(); dragX.snapTo(0f) }
                                else -> dragX.animateTo(0f, tween(160))
                            }
                        }
                    },
                    onDragCancel = { scope.launch { dragX.animateTo(0f) } },
                ) { change, amount ->
                    change.consume()
                    scope.launch { dragX.snapTo(dragX.value + amount) }
                }
            }
            .pointerInput(Unit) {
                var total = 0f
                detectVerticalDragGestures(
                    onDragStart = { total = 0f },
                    onDragEnd = { if (total < -24.dp.toPx()) onOpen() },
                ) { change, amount ->
                    change.consume()
                    total += amount
                }
            }
            .clickable(onClickLabel = "Open Now Playing", onClick = onOpen)
            .semantics {
                customActions = listOf(
                    CustomAccessibilityAction("Previous") { onPrevious(); true },
                    CustomAccessibilityAction("Next") { onNext(); true },
                )
            },
    ) {
        Row(
            Modifier
                .graphicsLayer {
                    translationX = dragX.value
                    alpha = 1f - (abs(dragX.value) / size.width.coerceAtLeast(1f)).coerceIn(0f, 0.6f)
                }
                .padding(start = 8.dp, top = 8.dp, bottom = 6.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            CoverImage(np.coverArt, Modifier.size(44.dp), shape = RoundedCornerShape(6.dp))
            Column(Modifier.weight(1f)) {
                Text(np.title, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (np.artist.isNotEmpty()) {
                    Text(np.artist, style = MaterialTheme.typography.bodySmall, color = palette.textDim, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
            IconButton(onClick = onTogglePlay, modifier = Modifier.size(48.dp)) {
                Icon(
                    if (np.playingOrWill) Icons.Rounded.Pause else Icons.Rounded.PlayArrow,
                    contentDescription = if (np.playingOrWill) "Pause" else "Play",
                    modifier = Modifier.size(30.dp),
                )
            }
            IconButton(onClick = onNext, enabled = np.hasNext, modifier = Modifier.size(48.dp)) {
                Icon(Icons.Rounded.SkipNext, contentDescription = "Next")
            }
        }
    }
}
