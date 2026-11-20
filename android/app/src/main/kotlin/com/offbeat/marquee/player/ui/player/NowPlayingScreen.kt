package com.offbeat.marquee.player.ui.player

import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.QueueMusic
import androidx.compose.material.icons.rounded.Bedtime
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material.icons.rounded.Favorite
import androidx.compose.material.icons.rounded.FavoriteBorder
import androidx.compose.material.icons.rounded.Lyrics
import androidx.compose.material.icons.rounded.MoreHoriz
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Repeat
import androidx.compose.material.icons.rounded.RepeatOne
import androidx.compose.material.icons.rounded.Shuffle
import androidx.compose.material.icons.rounded.SkipNext
import androidx.compose.material.icons.rounded.SkipPrevious
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import com.offbeat.marquee.player.eq.EqStore
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.max
import androidx.compose.ui.unit.min
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.media3.common.Player
import com.offbeat.marquee.core.lyrics.LyricsTrack
import com.offbeat.marquee.player.data.LyricsRepo
import com.offbeat.marquee.player.settings.UiSettings
import com.offbeat.marquee.player.art.Backdrop
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.playback.FavouriteResult
import com.offbeat.marquee.player.playback.NowPlaying
import com.offbeat.marquee.player.playback.PlayerConnection
import com.offbeat.marquee.player.playback.QueueRow
import com.offbeat.marquee.player.playback.SleepState
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import androidx.compose.runtime.saveable.rememberSaveable
import com.offbeat.marquee.player.ui.components.ThemedSnackbarHost

/**
 * Now Playing: a full-screen sheet over the blurred cover. Swipe it down or go Back (predictive back
 * shrinks it as the gesture goes) to close it. Portrait: the cover, then title, seek bar and
 * controls in the lower part. Landscape: the cover on the left at full height, the rest on the right.
 * The Lyrics button puts the lyrics in the cover's place in both (see [LyricsPanel]).
 */
@Composable
fun NowPlayingScreen(
    np: NowPlaying,
    queue: List<QueueRow>,
    player: PlayerConnection,
    onClose: () -> Unit,
    onOpenAlbum: (String) -> Unit,
    onOpenArtist: (String) -> Unit,
    onOpenEqualizer: () -> Unit,
    snackbar: SnackbarHostState,
) {
    val palette = Palette.current
    val scope = rememberCoroutineScope()
    val density = LocalDensity.current
    val dragY = remember { Animatable(0f) }
    var backProgress by remember { mutableFloatStateOf(0f) }
    var height by remember { mutableIntStateOf(1) }
    // Saveable: an open sheet stays open across a rotation.
    var showQueue by rememberSaveable { mutableStateOf(false) }
    var showSleep by rememberSaveable { mutableStateOf(false) }
    // Where the controls start, so snackbars sit above them rather than over the bottom row.
    var controlsTop by remember { mutableFloatStateOf(0f) }
    val aboveControls = Modifier.onGloballyPositioned { controlsTop = it.positionInRoot().y }

    // A text field left focused underneath (Search) would bring the keyboard back over Now Playing,
    // for example after a rotation.
    val focusManager = LocalFocusManager.current
    LaunchedEffect(Unit) { focusManager.clearFocus(force = true) }

    // Lyrics: shown in place of the cover, remembered between openings. Looked up only while shown
    // and the app is in the foreground; the next track's once, ahead, while the view stays open.
    val lyricsOpen by UiSettings.lyricsOpen.collectAsStateWithLifecycle()
    val eq by EqStore.setting.collectAsStateWithLifecycle()
    val lyricsVm: LyricsViewModel = viewModel()
    val lyricsState by lyricsVm.state.collectAsStateWithLifecycle()
    val lifecycleState by LocalLifecycleOwner.current.lifecycle.currentStateFlow.collectAsState()
    val lyricsActive = lyricsOpen && lifecycleState.isAtLeast(Lifecycle.State.STARTED)
    val tracks = remember(np.songId, np.index, queue) { lyricsTracks(np, queue) }
    LaunchedEffect(lyricsActive, tracks) {
        val (cur, next) = tracks
        if (lyricsActive && cur != null) lyricsVm.show(cur, next) else lyricsVm.hide()
    }
    DisposableEffect(lyricsVm) { onDispose { lyricsVm.hide() } }
    val lyrics = @Composable { modifier: Modifier ->
        LyricsPanel(
            state = lyricsState,
            np = np,
            onSeek = { player.seekTo(it) },
            onRefresh = lyricsVm::refresh,
            onSearchOnline = lyricsVm::searchOnline,
            onRetry = lyricsVm::retry,
            modifier = modifier,
        )
    }
    val toggleLyrics = { UiSettings.setLyricsOpen(!lyricsOpen) }
    val eqShown = eq.enabled

    PredictiveBackHandler { progress ->
        try {
            progress.collect { backProgress = it.progress }
            onClose()
        } catch (e: CancellationException) {
            backProgress = 0f
            throw e
        }
    }

    val onFavourite: () -> Unit = remember(player, scope) {
        {
            scope.launch {
                when (player.toggleFavourite()) {
                    FavouriteResult.ReadOnly -> Library.postReadOnly()
                    FavouriteResult.Failed -> Library.post("Couldn't change the favourite")
                    FavouriteResult.Done -> Unit
                }
            }
        }
    }

    Box(
        Modifier
            .fillMaxSize()
            .onSizeChanged { height = it.height }
            .graphicsLayer {
                val p = backProgress
                translationY = dragY.value + p * with(density) { 56.dp.toPx() }
                scaleX = 1f - 0.08f * p
                scaleY = 1f - 0.08f * p
                shape = RoundedCornerShape(28.dp * p)
                clip = p > 0f || dragY.value > 0f
            }
            .background(palette.bg)
            .draggable(
                state = rememberDraggableState { d -> scope.launch { dragY.snapTo((dragY.value + d).coerceAtLeast(0f)) } },
                orientation = Orientation.Vertical,
                onDragStopped = { velocity ->
                    if (dragY.value > height * 0.18f || velocity > 2000f) onClose() else dragY.animateTo(0f, tween(200))
                },
            ),
    ) {
        Backdrop(np.coverArt, palette.backdropTint, Modifier.fillMaxSize())
        BoxWithConstraints(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing)) {
            if (maxWidth > maxHeight) {
                // Landscape: the cover at full height on the left, everything else on the right.
                // A short window (a 16:9 phone on its side: 360 dp, less the status bar) gets a one-line
                // title and a smaller play button, or the bottom row (lyrics, queue) fell off the screen.
                val short = maxHeight < 400.dp
                // The cover is as tall as the window, but leaves the controls at least 300 dp of width:
                // a full-height cover on a 640 dp wide phone squeezed them until repeat was cut off.
                val coverSize = max(96.dp, min(maxHeight - 32.dp, maxWidth - 32.dp - 24.dp - 300.dp))
                Row(
                    Modifier.fillMaxSize().padding(16.dp),
                    horizontalArrangement = Arrangement.spacedBy(24.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    // Lyrics take the cover's place, so the title and controls stay where they are and
                    // stay in reach (pause, seek) while reading.
                    if (lyricsOpen) lyrics(Modifier.fillMaxHeight().width(coverSize))
                    else SwipeCover(np, player, Modifier.size(coverSize))
                    Column(Modifier.weight(1f).fillMaxHeight(), verticalArrangement = Arrangement.SpaceBetween) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Spacer(Modifier.weight(1f))
                            if (eqShown) EqMarker(eq.label, onOpenEqualizer)
                            CloseButton(onClose)
                        }
                        TitleBlock(np, onFavourite, onOpenAlbum, onOpenArtist, compact = short)
                        SeekBar(np, { player.seekTo(it) }, aboveControls)
                        Controls(np, player, compact = short)
                        BottomRow(np, lyricsOpen, toggleLyrics, onQueue = { showQueue = true }, onSleep = { showSleep = true }, onOpenEqualizer, onOpenAlbum, onOpenArtist)
                    }
                }
            } else {
                // At most 600 dp wide, centred: on a tablet held upright (800 dp) the seek bar and the
                // controls otherwise spread across the whole width.
                Column(
                    Modifier.align(Alignment.TopCenter).fillMaxHeight().widthIn(max = 600.dp).fillMaxWidth()
                        .padding(horizontal = 24.dp),
                ) {
                    Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        CloseButton(onClose)
                        Text(
                            "Now playing",
                            style = MaterialTheme.typography.labelSmall,
                            color = palette.textDim,
                            modifier = Modifier.weight(1f),
                            textAlign = TextAlign.Center,
                        )
                        // As wide as the close button, so the title stays centred.
                        Box(Modifier.width(48.dp), contentAlignment = Alignment.Center) {
                            if (eqShown) EqMarker(eq.label, onOpenEqualizer)
                        }
                    }
                    BoxWithConstraints(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                        if (lyricsOpen) lyrics(Modifier.fillMaxSize().padding(vertical = 8.dp))
                        else SwipeCover(np, player, Modifier.size(min(maxWidth, maxHeight - 16.dp)))
                    }
                    TitleBlock(np, onFavourite, onOpenAlbum, onOpenArtist)
                    Spacer(Modifier.height(12.dp))
                    SeekBar(np, { player.seekTo(it) }, aboveControls)
                    Spacer(Modifier.height(4.dp))
                    Controls(np, player)
                    Spacer(Modifier.height(16.dp))
                    BottomRow(np, lyricsOpen, toggleLyrics, onQueue = { showQueue = true }, onSleep = { showSleep = true }, onOpenEqualizer, onOpenAlbum, onOpenArtist)
                    Spacer(Modifier.height(8.dp))
                }
            }
        }
    }

    ThemedSnackbarHost(
        snackbar,
        Modifier
            .fillMaxSize()
            .wrapContentHeight(Alignment.Bottom)
            .padding(bottom = with(density) { (height - controlsTop).coerceAtLeast(0f).toDp() } + 4.dp),
    )

    if (showQueue) QueueSheet(queue, np.index, player, onDismiss = { showQueue = false })
    if (showSleep) SleepSheet(np.sleep, onChoose = { player.setSleepTimer(it); showSleep = false }, onDismiss = { showSleep = false })
}

/** The current track and the next one in play order, as lyrics lookups take them. */
private fun lyricsTracks(np: NowPlaying, queue: List<QueueRow>): Pair<LyricsTrack?, LyricsTrack?> {
    if (!np.hasItem) return null to null
    val pos = queue.indexOfFirst { it.index == np.index }
    val row = queue.getOrNull(pos)?.takeIf { it.songId == np.songId }
    val cur = row?.let { LyricsRepo.trackOf(it.song, np.durationMs) }
        ?: LyricsTrack(np.songId, np.artist, np.title, np.album, np.durationMs / 1000.0)
    val next = if (row != null) queue.getOrNull(pos + 1)?.let { LyricsRepo.trackOf(it.song, it.durationMs) } else null
    return cur to next
}

@Composable
private fun CloseButton(onClose: () -> Unit) {
    IconButton(onClick = onClose, modifier = Modifier.size(48.dp)) {
        Icon(Icons.Rounded.ExpandMore, contentDescription = "Close Now Playing", modifier = Modifier.size(30.dp))
    }
}

/**
 * The big cover. Swiping it sideways skips (the old cover slides out, the new one in); a track
 * change from anywhere slides the same way, by direction in the queue.
 */
@Composable
private fun SwipeCover(np: NowPlaying, player: PlayerConnection, modifier: Modifier) {
    val scope = rememberCoroutineScope()
    val dragX = remember { Animatable(0f) }
    var lastIndex by remember { mutableIntStateOf(np.index) }
    val forward = np.index >= lastIndex
    SideEffect { lastIndex = np.index }
    Box(
        modifier
            .pointerInput(np.hasNext) {
                detectHorizontalDragGestures(
                    onDragEnd = {
                        val w = size.width.toFloat()
                        scope.launch {
                            when {
                                dragX.value < -w * 0.25f && np.hasNext -> { dragX.animateTo(-w, tween(140)); player.next(); dragX.snapTo(0f) }
                                dragX.value > w * 0.25f -> { dragX.animateTo(w, tween(140)); player.previous(); dragX.snapTo(0f) }
                                else -> dragX.animateTo(0f, tween(180))
                            }
                        }
                    },
                    onDragCancel = { scope.launch { dragX.animateTo(0f) } },
                ) { change, amount ->
                    change.consume()
                    scope.launch { dragX.snapTo(dragX.value + amount) }
                }
            }
            .graphicsLayer { translationX = dragX.value },
    ) {
        AnimatedContent(
            targetState = np.songId to np.coverArt,
            transitionSpec = {
                val dir = if (forward) 1 else -1
                (slideInHorizontally(tween(220)) { it * dir } + fadeIn(tween(220))) togetherWith
                    (slideOutHorizontally(tween(220)) { -it * dir } + fadeOut(tween(160))) using SizeTransform(clip = false)
            },
            label = "cover",
        ) { (_, cover) ->
            CoverImage(
                cover,
                Modifier.fillMaxSize(),
                shape = RoundedCornerShape(12.dp),
                contentDescription = "Cover of ${np.album}",
            )
        }
    }
}

@Composable
private fun TitleBlock(
    np: NowPlaying,
    onFavourite: () -> Unit,
    onOpenAlbum: (String) -> Unit,
    onOpenArtist: (String) -> Unit,
    compact: Boolean = false,
) {
    val palette = Palette.current
    Row(Modifier.fillMaxWidth().padding(top = if (compact) 0.dp else 16.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(
                np.title,
                style = MaterialTheme.typography.headlineSmall,
                maxLines = if (compact) 1 else 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.semantics { heading() },
            )
            Text(
                np.artist,
                style = MaterialTheme.typography.titleMedium,
                color = palette.text.copy(alpha = 0.85f),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier
                    .clickable(enabled = np.artistId != null, role = Role.Button, onClickLabel = "Open artist") { np.artistId?.let(onOpenArtist) }
                    .padding(vertical = 4.dp),
            )
            if (np.album.isNotEmpty()) {
                Text(
                    np.album,
                    style = MaterialTheme.typography.bodyMedium,
                    color = palette.textDim,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier
                        .clickable(enabled = np.albumId != null, role = Role.Button, onClickLabel = "Open album") { np.albumId?.let(onOpenAlbum) }
                        .padding(vertical = 4.dp),
                )
            }
        }
        IconButton(onClick = onFavourite, modifier = Modifier.size(48.dp)) {
            Icon(
                if (np.favourite) Icons.Rounded.Favorite else Icons.Rounded.FavoriteBorder,
                contentDescription = if (np.favourite) "Remove from favourites" else "Add to favourites",
                tint = if (np.favourite) palette.accent else palette.text,
            )
        }
    }
}

@Composable
private fun Controls(np: NowPlaying, player: PlayerConnection, compact: Boolean = false) {
    val palette = Palette.current
    val skip = if (compact) 48.dp else 56.dp
    val play = if (compact) 56.dp else 72.dp
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = player::toggleShuffle, modifier = Modifier.size(48.dp)) {
            Icon(
                Icons.Rounded.Shuffle,
                contentDescription = if (np.shuffle) "Shuffle on" else "Shuffle off",
                tint = if (np.shuffle) palette.accent else palette.textDim,
            )
        }
        IconButton(onClick = { player.previous() }, modifier = Modifier.size(skip)) {
            Icon(Icons.Rounded.SkipPrevious, contentDescription = "Previous", modifier = Modifier.size(36.dp))
        }
        FilledIconButton(
            onClick = player::togglePlay,
            modifier = Modifier.size(play),
            shape = CircleShape,
            colors = IconButtonDefaults.filledIconButtonColors(containerColor = palette.accent, contentColor = palette.onAccent),
        ) {
            Icon(
                if (np.playingOrWill) Icons.Rounded.Pause else Icons.Rounded.PlayArrow,
                contentDescription = if (np.playingOrWill) "Pause" else "Play",
                modifier = Modifier.size(if (compact) 32.dp else 40.dp),
            )
        }
        IconButton(onClick = { player.next() }, enabled = np.hasNext, modifier = Modifier.size(skip)) {
            Icon(Icons.Rounded.SkipNext, contentDescription = "Next", modifier = Modifier.size(36.dp))
        }
        IconButton(onClick = player::cycleRepeat, modifier = Modifier.size(48.dp)) {
            val (icon, label) = when (np.repeatMode) {
                Player.REPEAT_MODE_ONE -> Icons.Rounded.RepeatOne to "Repeat one"
                Player.REPEAT_MODE_ALL -> Icons.Rounded.Repeat to "Repeat all"
                else -> Icons.Rounded.Repeat to "Repeat off"
            }
            Icon(icon, contentDescription = label, tint = if (np.repeatMode != Player.REPEAT_MODE_OFF) palette.accent else palette.textDim)
        }
    }
}

/**
 * A small lit "EQ" while the equalizer is on (like the desktop's lit button), so the sound never looks
 * untouched when it isn't. Opens the equalizer; 48 dp to touch.
 */
@Composable
private fun EqMarker(label: String, onClick: () -> Unit) {
    val palette = Palette.current
    Box(
        Modifier
            .size(48.dp)
            .clickable(role = Role.Button, onClickLabel = "Open the equalizer", onClick = onClick)
            .semantics { contentDescription = "Equalizer on: $label" },
        contentAlignment = Alignment.Center,
    ) {
        Text(
            "EQ",
            style = MaterialTheme.typography.labelSmall,
            color = palette.accent,
            modifier = Modifier
                .border(1.dp, palette.accent.copy(alpha = 0.6f), RoundedCornerShape(4.dp))
                .padding(horizontal = 5.dp, vertical = 1.dp),
        )
    }
}

/** Lyrics (on or off), Queue, Sleep timer and ⋯ (go to the album or artist, the equalizer). */
@Composable
private fun BottomRow(
    np: NowPlaying,
    lyricsOpen: Boolean,
    onLyrics: () -> Unit,
    onQueue: () -> Unit,
    onSleep: () -> Unit,
    onEqualizer: () -> Unit,
    onOpenAlbum: (String) -> Unit,
    onOpenArtist: (String) -> Unit,
) {
    val palette = Palette.current
    var menu by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
        IconButton(
            onClick = onLyrics,
            modifier = Modifier.size(48.dp).semantics { stateDescription = if (lyricsOpen) "Showing" else "Hidden" },
        ) {
            Icon(Icons.Rounded.Lyrics, contentDescription = "Lyrics", tint = if (lyricsOpen) palette.accent else palette.textDim)
        }
        IconButton(onClick = onQueue, modifier = Modifier.size(48.dp)) {
            Icon(Icons.AutoMirrored.Rounded.QueueMusic, contentDescription = "Queue", tint = palette.textDim)
        }
        SleepButton(np.sleep, onSleep)
        Box {
            IconButton(onClick = { menu = true }, modifier = Modifier.size(48.dp)) {
                Icon(Icons.Rounded.MoreHoriz, contentDescription = "More", tint = palette.textDim)
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                DropdownMenuItem(
                    text = { Text("Go to album") },
                    enabled = np.albumId != null,
                    onClick = { menu = false; np.albumId?.let(onOpenAlbum) },
                )
                DropdownMenuItem(
                    text = { Text("Go to artist") },
                    enabled = np.artistId != null,
                    onClick = { menu = false; np.artistId?.let(onOpenArtist) },
                )
                DropdownMenuItem(
                    text = { Text("Equalizer") },
                    onClick = { menu = false; onEqualizer() },
                )
            }
        }
    }
}

/** The sleep button; with a timer set it is lit and shows the time left. */
@Composable
private fun SleepButton(sleep: SleepState, onClick: () -> Unit) {
    val palette = Palette.current
    val on = sleep != SleepState.Off
    val left = sleepLabel(sleep, short = true)
    val full = sleepLabel(sleep, short = false) ?: "Off"
    Row(
        Modifier
            .height(48.dp)
            .clickable(role = Role.Button, onClickLabel = "Sleep timer", onClick = onClick)
            .padding(horizontal = 12.dp)
            .semantics { stateDescription = full },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Rounded.Bedtime, contentDescription = "Sleep timer", tint = if (on) palette.accent else palette.textDim)
        if (left != null) {
            Spacer(Modifier.width(6.dp))
            Text(left, style = MaterialTheme.typography.labelMedium, color = palette.accent)
        }
    }
}
