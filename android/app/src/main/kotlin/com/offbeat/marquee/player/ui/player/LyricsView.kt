package com.offbeat.marquee.player.ui.player

import android.os.SystemClock
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.animateScrollBy
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.MyLocation
import androidx.compose.material3.AssistChip
import androidx.compose.material3.AssistChipDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.offbeat.marquee.core.lyrics.Lyrics
import com.offbeat.marquee.core.lyrics.LyricsSource
import com.offbeat.marquee.player.playback.NowPlaying
import com.offbeat.marquee.player.ui.components.formatTime
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first

/**
 * The lyrics in Now Playing, in place of the cover. Synced lyrics follow playback (the current line
 * in the accent, a third of the way down); a tap on a line plays from it; scrolling by hand stops the
 * following until "Back to current line" or 5 s of rest. Plain lyrics just scroll. Pulling down asks
 * the server again.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LyricsPanel(
    state: LyricsViewState,
    np: NowPlaying,
    onSeek: (Long) -> Unit,
    onRefresh: () -> Unit,
    onSearchOnline: () -> Unit,
    onRetry: () -> Unit,
    modifier: Modifier = Modifier,
) {
    // A state for another song (the view model has not caught up with a skip yet) shows as loading.
    val ui = if (state.songId == np.songId) state.ui else LyricsUi.Loading
    PullToRefreshBox(isRefreshing = state.refreshing, onRefresh = onRefresh, modifier = modifier) {
        when (ui) {
            LyricsUi.Loading -> Message(busy = true, text = "Looking for lyrics…")
            is LyricsUi.Shown -> Box(Modifier.fillMaxSize()) {
                if (ui.lyrics.synced) SyncedLyrics(ui.lyrics, np, onSeek) else PlainLyrics(ui.lyrics)
            }
            LyricsUi.Instrumental -> Message(text = "Instrumental", note = "From LRCLIB")
            is LyricsUi.NotFound -> Message(
                text = "No lyrics found",
                note = ui.searchReason,
                action = "Search online",
                actionEnabled = ui.searchReason == null,
                onAction = onSearchOnline,
            )
            is LyricsUi.Failed -> Message(text = ui.message, action = "Try again", onAction = onRetry)
        }
    }
}

/** A centred state: loading, nothing found, or an error. Scrollable, so pull to refresh works on it. */
@Composable
private fun Message(
    text: String,
    busy: Boolean = false,
    note: String? = null,
    action: String? = null,
    actionEnabled: Boolean = true,
    onAction: () -> Unit = {},
) {
    val palette = Palette.current
    BoxWithConstraints(Modifier.fillMaxSize()) {
        Column(
            Modifier.fillMaxWidth().heightIn(min = maxHeight).verticalScroll(rememberScrollState()).padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            if (busy) {
                CircularProgressIndicator(Modifier.size(20.dp), color = palette.textFaint, strokeWidth = 2.dp)
                Spacer(Modifier.height(12.dp))
                Text(text, style = MaterialTheme.typography.bodyMedium, color = palette.textFaint)
                return@Column
            }
            Text(text, style = MaterialTheme.typography.titleMedium, color = palette.text, textAlign = TextAlign.Center)
            if (note != null) {
                Spacer(Modifier.height(6.dp))
                Text(note, style = MaterialTheme.typography.bodySmall, color = palette.textDim, textAlign = TextAlign.Center)
            }
            if (action != null) {
                Spacer(Modifier.height(16.dp))
                OutlinedButton(onClick = onAction, enabled = actionEnabled, modifier = Modifier.heightIn(min = 48.dp)) { Text(action) }
            }
        }
    }
}

/** A small "From your server" / "From LRCLIB" under the lyrics. */
@Composable
private fun SourceLabel(source: LyricsSource, modifier: Modifier = Modifier) {
    Text(
        if (source == LyricsSource.Server) "From your server" else "From LRCLIB",
        style = MaterialTheme.typography.labelSmall,
        color = Palette.current.textFaint,
        modifier = modifier,
    )
}

/** Fades the list out at its top and bottom edges (drawn once per frame of scrolling, no blur). */
private fun Modifier.fadedEdges(): Modifier = this
    .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
    .drawWithContent {
        drawContent()
        drawRect(
            Brush.verticalGradient(0f to Color.Transparent, 0.08f to Color.Black, 0.88f to Color.Black, 1f to Color.Transparent),
            blendMode = BlendMode.DstIn,
        )
    }

@Composable
private fun PlainLyrics(lyrics: Lyrics) {
    val palette = Palette.current
    Column(Modifier.fillMaxSize()) {
        LazyColumn(Modifier.weight(1f).fillMaxWidth().fadedEdges(), contentPadding = PaddingValues(vertical = 24.dp)) {
            itemsIndexed(lyrics.lines, contentType = { _, _ -> "line" }) { _, line ->
                Text(
                    line.text,
                    style = MaterialTheme.typography.bodyLarge.copy(fontSize = 18.sp, lineHeight = 26.sp),
                    color = palette.text.copy(alpha = 0.9f),
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
                )
            }
        }
        SourceLabel(lyrics.source, Modifier.padding(start = 4.dp, top = 4.dp))
    }
}

/** How long auto-follow stays off after scrolling by hand. */
private const val FOLLOW_PAUSE_MS = 5_000L

@Composable
private fun SyncedLyrics(lyrics: Lyrics, np: NowPlaying, onSeek: (Long) -> Unit) {
    val palette = Palette.current
    val list = rememberLazyListState()

    // The line being sung: recomputed from the position snapshot when it changes, and otherwise only
    // when the next line is due (one wake-up per line, not per frame). Waiting for a frame first means
    // nothing advances while the view is not drawn (the screen is off or the app is in the background).
    var current by remember(lyrics) { mutableIntStateOf(lyrics.currentLine(np.positionAt(SystemClock.elapsedRealtime()))) }
    LaunchedEffect(lyrics, np.isPlaying, np.positionMs, np.snapshotAtElapsedMs, np.speed) {
        while (true) {
            val pos = np.positionAt(SystemClock.elapsedRealtime())
            current = lyrics.currentLine(pos)
            if (!np.isPlaying) break
            val due = lyrics.nextChangeAt(pos) ?: break
            delay(((due - pos) / np.speed.coerceAtLeast(0.1f)).toLong().coerceAtLeast(0) + 2)
            withFrameMillis { }
        }
    }

    // Scrolling by hand pauses the following.
    var following by remember { mutableStateOf(true) }
    val lastUserScroll = remember { LongArray(1) }
    val userScroll = remember {
        object : NestedScrollConnection {
            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                if (source == NestedScrollSource.UserInput) {
                    lastUserScroll[0] = SystemClock.uptimeMillis()
                    following = false
                }
                return Offset.Zero
            }
        }
    }
    LaunchedEffect(following) {
        if (following) return@LaunchedEffect
        while (true) {
            snapshotFlow { list.isScrollInProgress }.first { !it }
            val wait = lastUserScroll[0] + FOLLOW_PAUSE_MS - SystemClock.uptimeMillis()
            if (wait <= 0) break
            delay(wait)
        }
        following = true
    }

    Column(Modifier.fillMaxSize()) {
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
            val viewport = maxHeight
            // The current line sits about a third of the way down: the list starts that far down, so
            // scrolling a line to the top of the content puts it there; the bottom padding lets the
            // last line get there too.
            val top = viewport * 0.3f
            LaunchedEffect(current, following, lyrics) {
                if (!following) return@LaunchedEffect
                val target = current.coerceAtLeast(0)
                val info = list.layoutInfo.visibleItemsInfo.firstOrNull { it.index == target }
                if (info != null) {
                    list.animateScrollBy(info.offset.toFloat(), tween(450, easing = FastOutSlowInEasing))
                } else {
                    list.scrollToItem(target)
                }
            }
            LazyColumn(
                state = list,
                modifier = Modifier.fillMaxSize().nestedScroll(userScroll).fadedEdges(),
                contentPadding = PaddingValues(top = top, bottom = viewport * 0.6f),
            ) {
                itemsIndexed(lyrics.lines, contentType = { _, _ -> "line" }) { i, line ->
                    val state = when {
                        i == current -> LineState.Current
                        i < current -> LineState.Past
                        else -> LineState.Upcoming
                    }
                    LyricLineText(
                        text = line.text,
                        state = state,
                        timeLabel = lyrics.lineStart(i)?.let(::formatTime),
                        onClick = {
                            lyrics.lineStart(i)?.let(onSeek)
                            current = i
                            following = true
                        },
                    )
                }
            }
            // Fades in when following stops (scrolling by hand) and out when it resumes.
            androidx.compose.animation.AnimatedVisibility(
                visible = !following,
                modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 4.dp),
                enter = androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(200)),
                exit = androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(160)),
            ) {
                AssistChip(
                    onClick = { following = true },
                    label = { Text("Back to current line") },
                    leadingIcon = { Icon(Icons.Rounded.MyLocation, contentDescription = null, modifier = Modifier.size(18.dp)) },
                    colors = AssistChipDefaults.assistChipColors(
                        containerColor = palette.surfaceHi,
                        labelColor = palette.text,
                        leadingIconContentColor = palette.accent,
                    ),
                    border = null,
                )
            }
        }
        SourceLabel(lyrics.source, Modifier.padding(start = 4.dp, top = 4.dp))
    }
}

private enum class LineState { Past, Current, Upcoming }

@Composable
private fun LyricLineText(text: String, state: LineState, timeLabel: String?, onClick: () -> Unit) {
    val palette = Palette.current
    val color by animateColorAsState(
        when (state) {
            LineState.Current -> palette.accent
            LineState.Past -> palette.text.copy(alpha = 0.38f)
            LineState.Upcoming -> palette.text.copy(alpha = 0.72f)
        },
        tween(250),
        label = "lineColor",
    )
    val scale by animateFloatAsState(if (state == LineState.Current) 1.06f else 1f, tween(250), label = "lineScale")
    Text(
        text.ifBlank { "♪" },
        style = MaterialTheme.typography.titleLarge.copy(fontSize = 22.sp, lineHeight = 30.sp),
        fontWeight = if (state == LineState.Current) FontWeight.Bold else FontWeight.SemiBold,
        color = color,
        modifier = Modifier
            .fillMaxWidth()
            .clickable(role = Role.Button, onClickLabel = timeLabel?.let { "Play from $it" }, onClick = onClick)
            // Room on the right for the current line's slight enlargement.
            .padding(start = 4.dp, end = 24.dp, top = 8.dp, bottom = 8.dp)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
                transformOrigin = TransformOrigin(0f, 0.5f)
            }
            .semantics { selected = state == LineState.Current },
    )
}
