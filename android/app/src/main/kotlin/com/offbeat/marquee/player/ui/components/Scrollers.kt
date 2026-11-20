package com.offbeat.marquee.player.ui.components

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.drag
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.offbeat.marquee.core.library.AlbumPaging
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged

/**
 * A fast scroller for a grid of [total] cells: a thumb on the right edge that shows while the grid
 * scrolls; dragging it jumps the grid (the cells there are fetched as they come into view), and
 * [bubble] says where the thumb is (a letter, a year) while it is held. The thumb only takes
 * touches while it shows, so the cells under it stay tappable.
 *
 * [onDragging] lets the screen hold back page fetches until the thumb settles.
 */
@Composable
fun FastScroller(
    state: LazyGridState,
    total: Int,
    bubble: ((Int) -> String?)?,
    modifier: Modifier = Modifier,
    onDragging: (Boolean) -> Unit = {},
) {
    if (total <= 0) return
    val palette = Palette.current
    val density = LocalDensity.current
    val thumbH = with(density) { 52.dp.toPx() }
    var trackH by remember { mutableIntStateOf(0) }
    var dragging by remember { mutableStateOf(false) }
    var dragFraction by remember { mutableFloatStateOf(0f) }
    var recentlyScrolled by remember { mutableStateOf(false) }
    val scrolling by remember { derivedStateOf { state.isScrollInProgress } }
    LaunchedEffect(scrolling, dragging) {
        if (scrolling || dragging) recentlyScrolled = true else { delay(1200); recentlyScrolled = false }
    }
    val shown = dragging || recentlyScrolled
    val alpha by animateFloatAsState(if (shown) 1f else 0f, tween(if (shown) 120 else 400), label = "scroller")
    // Only worth having when the list is a few screens long.
    val long by remember(total) { derivedStateOf { state.layoutInfo.visibleItemsInfo.size.let { it > 0 && total > it * 3 } } }
    if (!long) return

    fun fraction(): Float = if (dragging) dragFraction else {
        val info = state.layoutInfo
        AlbumPaging.fractionOf(state.firstVisibleItemIndex, info.visibleItemsInfo.size, total)
    }

    LaunchedEffect(state, total) {
        snapshotFlow { if (dragging) AlbumPaging.indexAt(dragFraction, total) else -1 }
            .distinctUntilChanged()
            .collectLatest { if (it >= 0) state.scrollToItem(it) }
    }

    BoxWithConstraints(modifier.fillMaxHeight().width(120.dp).onSizeChanged { trackH = it.height }) {
        val travel = (trackH - thumbH).coerceAtLeast(1f)
        // The bubble, beside the thumb while it is held.
        if (dragging && bubble != null) {
            val text = bubble(AlbumPaging.indexAt(dragFraction, total))
            if (text != null) {
                Box(
                    Modifier
                        .align(Alignment.TopEnd)
                        .offset { IntOffset(-with(density) { 44.dp.roundToPx() }, (fraction() * travel + thumbH / 2 - with(density) { 32.dp.toPx() }).toInt()) }
                        .size(64.dp)
                        .background(palette.accent, RoundedCornerShape(topStartPercent = 50, topEndPercent = 50, bottomStartPercent = 50, bottomEndPercent = 10))
                        .semantics { contentDescription = text },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        text,
                        color = palette.onAccent,
                        fontSize = if (text.length > 2) 18.sp else 28.sp,
                        style = MaterialTheme.typography.headlineMedium,
                        textAlign = TextAlign.Center,
                    )
                }
            }
        }
        Box(
            Modifier
                .align(Alignment.TopEnd)
                .offset { IntOffset(0, (fraction() * travel).toInt()) }
                .size(width = 36.dp, height = 52.dp)
                .alpha(alpha)
                .then(
                    if (!shown) Modifier else Modifier.pointerInput(total, trackH) {
                        awaitEachGesture {
                            val down = awaitFirstDown(requireUnconsumed = false)
                            down.consume()
                            // Positions are in the thumb's own coordinates, and the thumb moves
                            // with the drag: add where it is drawn to get a fixed frame.
                            val grab = down.position.y
                            dragFraction = fraction()
                            dragging = true
                            onDragging(true)
                            try {
                                drag(down.id) { change ->
                                    val top = dragFraction * travel + change.position.y - grab
                                    dragFraction = (top / travel).coerceIn(0f, 1f)
                                    change.consume()
                                }
                            } finally {
                                dragging = false
                                onDragging(false)
                            }
                        }
                    },
                )
                .semantics { contentDescription = "Fast scroll" },
            contentAlignment = Alignment.CenterEnd,
        ) {
            Box(
                Modifier
                    .padding(end = 4.dp)
                    .size(width = if (dragging) 8.dp else 6.dp, height = 44.dp)
                    .background(if (dragging) palette.accent else palette.textDim.copy(alpha = 0.8f), RoundedCornerShape(4.dp)),
            )
        }
    }
}

/**
 * The letters down the right edge of a list: touching or dragging along them jumps to that letter,
 * with the letter shown large beside the finger.
 */
@Composable
fun AlphabetScroller(letters: List<String>, onLetter: (Int) -> Unit, onHeld: (String?) -> Unit, modifier: Modifier = Modifier) {
    if (letters.size < 2) return
    val palette = Palette.current
    var height by remember { mutableIntStateOf(0) }
    var active by remember { mutableIntStateOf(-1) }
    val density = LocalDensity.current
    Box(modifier.fillMaxHeight().width(28.dp)) {
        Column(
            Modifier
                .fillMaxHeight()
                .fillMaxWidth()
                .onSizeChanged { height = it.height }
                .pointerInput(letters) {
                    fun pick(y: Float) {
                        val i = ((y / height.coerceAtLeast(1)) * letters.size).toInt().coerceIn(0, letters.lastIndex)
                        if (i != active) {
                            active = i
                            onLetter(i)
                            onHeld(letters[i])
                        }
                    }
                    awaitEachGesture {
                        val down = awaitFirstDown()
                        down.consume()
                        pick(down.position.y)
                        drag(down.id) { change ->
                            pick(change.position.y)
                            change.consume()
                        }
                        active = -1
                        onHeld(null)
                    }
                }
                .semantics { contentDescription = "Jump to letter" },
            verticalArrangement = androidx.compose.foundation.layout.Arrangement.SpaceEvenly,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            // On a short window, every other letter (the touch still picks each one).
            val step = if (with(density) { height.toDp() } < (letters.size * 13).dp) 2 else 1
            letters.forEachIndexed { i, l ->
                if (i % step == 0) {
                    Text(
                        l,
                        fontSize = 10.sp,
                        lineHeight = 11.sp,
                        color = if (i == active) palette.accent else palette.textDim,
                        style = MaterialTheme.typography.labelSmall.copy(letterSpacing = 0.sp),
                    )
                }
            }
        }
    }
}

/** The big letter shown while the alphabet scroller is held, centred over the list. */
@Composable
fun LetterBubble(text: String, modifier: Modifier = Modifier) {
    val palette = Palette.current
    Box(modifier.size(72.dp).background(palette.accent, CircleShape), contentAlignment = Alignment.Center) {
        Text(text, color = palette.onAccent, fontSize = 30.sp, style = MaterialTheme.typography.headlineMedium)
    }
}


/**
 * A LazyListState that comes back to its exact place after the activity is recreated (a rotation),
 * even near the end of a short list: the first frames are laid out before the window insets and the
 * mini player are known, with a taller viewport, and a plain restored state is clamped to it. This
 * one keeps its own (index, offset) and puts the list back once those frames are past.
 */
@androidx.compose.runtime.Composable
fun rememberKeptListState(): androidx.compose.foundation.lazy.LazyListState {
    var index by androidx.compose.runtime.saveable.rememberSaveable { androidx.compose.runtime.mutableIntStateOf(0) }
    var offset by androidx.compose.runtime.saveable.rememberSaveable { androidx.compose.runtime.mutableIntStateOf(0) }
    val state = androidx.compose.runtime.remember { androidx.compose.foundation.lazy.LazyListState(index, offset) }
    androidx.compose.runtime.LaunchedEffect(state) {
        if (index != 0 || offset != 0) {
            repeat(3) { androidx.compose.runtime.withFrameNanos { } }
            if (state.firstVisibleItemIndex != index || state.firstVisibleItemScrollOffset != offset) state.scrollToItem(index, offset)
        }
        androidx.compose.runtime.snapshotFlow { state.firstVisibleItemIndex to state.firstVisibleItemScrollOffset }
            .collect { (i, o) -> index = i; offset = o }
    }
    return state
}

/** [rememberKeptListState] for a lazy grid. */
@androidx.compose.runtime.Composable
fun rememberKeptGridState(): androidx.compose.foundation.lazy.grid.LazyGridState {
    var index by androidx.compose.runtime.saveable.rememberSaveable { androidx.compose.runtime.mutableIntStateOf(0) }
    var offset by androidx.compose.runtime.saveable.rememberSaveable { androidx.compose.runtime.mutableIntStateOf(0) }
    val state = androidx.compose.runtime.remember { androidx.compose.foundation.lazy.grid.LazyGridState(index, offset) }
    androidx.compose.runtime.LaunchedEffect(state) {
        if (index != 0 || offset != 0) {
            repeat(3) { androidx.compose.runtime.withFrameNanos { } }
            if (state.firstVisibleItemIndex != index || state.firstVisibleItemScrollOffset != offset) state.scrollToItem(index, offset)
        }
        androidx.compose.runtime.snapshotFlow { state.firstVisibleItemIndex to state.firstVisibleItemScrollOffset }
            .collect { (i, o) -> index = i; offset = o }
    }
    return state
}

/** [rememberKeptListState] for a scrolling column (a page that is not a lazy list). */
@androidx.compose.runtime.Composable
fun rememberKeptScrollState(): androidx.compose.foundation.ScrollState {
    var value by androidx.compose.runtime.saveable.rememberSaveable { androidx.compose.runtime.mutableIntStateOf(0) }
    val state = androidx.compose.runtime.remember { androidx.compose.foundation.ScrollState(value) }
    androidx.compose.runtime.LaunchedEffect(state) {
        if (value != 0) {
            repeat(3) { androidx.compose.runtime.withFrameNanos { } }
            if (state.value != value) state.scrollTo(value)
        }
        androidx.compose.runtime.snapshotFlow { state.value }.collect { value = it }
    }
    return state
}
