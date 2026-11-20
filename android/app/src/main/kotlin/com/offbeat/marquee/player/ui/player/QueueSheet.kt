package com.offbeat.marquee.player.ui.player

import androidx.compose.animation.core.spring
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.DragHandle
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import com.offbeat.marquee.core.library.ActionPlace
import com.offbeat.marquee.core.playback.QueueOrder
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.data.Item
import com.offbeat.marquee.player.playback.PlayerConnection
import com.offbeat.marquee.player.playback.QueueRow
import com.offbeat.marquee.player.ui.actions.ActionRequest
import com.offbeat.marquee.player.ui.actions.LocalItemActions
import com.offbeat.marquee.player.ui.components.formatTime
import com.offbeat.marquee.player.ui.components.DarkSheetBars
import com.offbeat.marquee.player.ui.theme.Palette

/**
 * The queue in play order (the shuffle order while shuffle is on): what has played (dimmed), the
 * current track pinned at the top with "Up next" under it, then what comes. Drag a row by its
 * handle to move it, swipe it away to remove it, or clear everything upcoming. Tapping a row plays
 * it; a long press opens the action sheet. The service does the edits, so the saved queue and the
 * prefetcher follow them.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalFoundationApi::class)
@Composable
fun QueueSheet(queue: List<QueueRow>, current: Int, player: PlayerConnection, onDismiss: () -> Unit) {
    val palette = Palette.current
    val currentPos = queue.indexOfFirst { it.index == current }.coerceAtLeast(0)
    val history = if (queue.isEmpty()) emptyList() else queue.subList(0, currentPos)
    val now = queue.getOrNull(currentPos)
    // While a drag is under way, the upcoming rows' order as the drag has arranged it (entry ids).
    // The drag handlers keep the closures they started with, so everything they touch is state that
    // lives as long as the sheet, and the drop is worked out against the queue as it is then.
    var dragOrder by remember { mutableStateOf<List<Long>?>(null) }
    val live = if (queue.isEmpty()) emptyList() else queue.drop(currentPos + 1)
    val upcoming = dragOrder?.let { order -> live.sortedBy { row -> order.indexOf(row.uid).let { if (it < 0) Int.MAX_VALUE else it } } } ?: live
    val latestUpcoming = rememberUpdatedState(upcoming)
    var removed by remember(queue) { mutableStateOf(emptySet<Long>()) }
    val list = rememberLazyListState(initialFirstVisibleItemIndex = history.size)
    // The current track at the top on opening (the first layout can come before the sheet has its height).
    LaunchedEffect(Unit) { list.scrollToItem(history.size) }
    var dragUid by remember { mutableStateOf<Long?>(null) }
    var dragOffset by remember { mutableFloatStateOf(0f) }
    val haptics = LocalHapticFeedback.current
    val openActions = LocalItemActions.current

    fun moveIfPassed() {
        val uid = dragUid ?: return
        val items = list.layoutInfo.visibleItemsInfo
        val me = items.firstOrNull { it.key == "u$uid" } ?: return
        val centre = me.offset + me.size / 2 + dragOffset
        val target = items.firstOrNull { info ->
            info.key != me.key && (info.key as? String)?.startsWith("u") == true && centre.toInt() in info.offset until info.offset + info.size
        } ?: return
        val targetUid = (target.key as String).drop(1).toLong()
        val order = dragOrder ?: return
        val from = order.indexOf(uid)
        val to = order.indexOf(targetUid)
        if (from < 0 || to < 0) return
        dragOrder = order.toMutableList().apply { add(to, removeAt(from)) }
        dragOffset += me.offset - target.offset
    }

    // Dragging a row near the top or bottom edge of the list scrolls it, faster the closer to the
    // edge, so a row can be taken past what is on screen. Runs only during a drag, once per frame.
    val edgePx = with(LocalDensity.current) { 72.dp.toPx() }
    val maxStepPx = with(LocalDensity.current) { 14.dp.toPx() }
    LaunchedEffect(dragUid) {
        val uid = dragUid ?: return@LaunchedEffect
        while (true) {
            withFrameMillis { }
            val info = list.layoutInfo
            val me = info.visibleItemsInfo.firstOrNull { it.key == "u$uid" } ?: continue
            // Upward, the current track's pinned row covers the top of the list.
            val pinned = info.visibleItemsInfo.firstOrNull { it.key == "now" }?.let { it.offset + it.size } ?: info.viewportStartOffset
            val top = me.offset + dragOffset
            val bottom = top + me.size
            val step = when {
                bottom > info.viewportEndOffset - edgePx -> maxStepPx * ((bottom - (info.viewportEndOffset - edgePx)) / edgePx).coerceIn(0f, 1f)
                top < pinned + edgePx -> -maxStepPx * (((pinned + edgePx) - top) / edgePx).coerceIn(0f, 1f)
                else -> 0f
            }
            if (step == 0f) continue
            val scrolled = list.scrollBy(step)
            // The row stays under the finger: what the list moved, the row's offset takes back.
            dragOffset += scrolled
            moveIfPassed()
        }
    }

    fun endDrag(commit: Boolean) {
        val uid = dragUid ?: return
        val order = dragOrder
        if (commit && order != null) {
            val now = player.queue.value
            val nowPos = now.indexOfFirst { it.index == player.state.value.index }.coerceAtLeast(0)
            QueueOrder.dropMove(now.map { it.uid }, nowPos, order, uid)?.let { (from, to) -> player.moveInQueue(from, to) }
        }
        dragUid = null
        dragOrder = null
        dragOffset = 0f
    }

    // The dragged row left Up next (it became the current track, or was removed elsewhere): its
    // gesture went with it, so the drag ends here.
    LaunchedEffect(live) {
        val uid = dragUid ?: return@LaunchedEffect
        if (live.none { it.uid == uid }) {
            dragUid = null
            dragOrder = null
            dragOffset = 0f
        }
    }

    // Play next and Remove are built from the entry id when tapped (ActionRequest.withQueueCallbacks),
    // so they act on the queue as it is then, not as it was when the sheet opened.
    fun actionsFor(row: QueueRow, isCurrent: Boolean) = ActionRequest(
        item = Item.OfSong(row.song),
        place = ActionPlace(inQueue = true, isCurrent = isCurrent),
        queueUid = row.uid,
    )

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = palette.surface,
    ) {
        DarkSheetBars()
        Text(
            "Queue · ${queue.size} ${if (queue.size == 1) "track" else "tracks"}",
            style = MaterialTheme.typography.titleLarge,
            modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp).semantics { heading() },
        )
        LazyColumn(state = list, contentPadding = PaddingValues(bottom = 16.dp), modifier = Modifier.fillMaxSize()) {
            items(history.filter { it.uid !in removed }, key = { "h${it.uid}" }, contentType = { "row" }) { row ->
                Swipeable(onRemove = { removed = removed + row.uid; player.removeFromQueue(row.index) }, modifier = Modifier.animateItem()) {
                    QueueItem(row, dim = true, isCurrent = false, handle = null, onPlay = { player.playAt(row.index) }, onLongPress = { openActions(actionsFor(row, false)) })
                }
            }
            if (now != null) {
                stickyHeader(key = "now", contentType = "now") {
                    Column(Modifier.background(palette.surface)) {
                        QueueItem(now, dim = false, isCurrent = true, handle = null, onPlay = {}, onLongPress = { openActions(actionsFor(now, true)) })
                        Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                "Up next · ${upcoming.size - upcoming.count { it.uid in removed }}",
                                style = MaterialTheme.typography.labelLarge,
                                color = palette.textDim,
                                modifier = Modifier.weight(1f).semantics { heading() },
                            )
                            if (upcoming.isNotEmpty()) {
                                TextButton(onClick = { player.clearUpcoming() }) { Text("Clear upcoming") }
                            }
                        }
                    }
                }
            }
            items(upcoming.filter { it.uid !in removed }, key = { "u${it.uid}" }, contentType = { "row" }) { row ->
                val dragging = dragUid == row.uid
                val handle = Modifier.pointerInput(row.uid) {
                    detectDragGestures(
                        onDragStart = {
                            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                            dragOrder = latestUpcoming.value.map { it.uid }
                            dragUid = row.uid
                            dragOffset = 0f
                        },
                        onDrag = { change, amount ->
                            change.consume()
                            dragOffset += amount.y
                            moveIfPassed()
                        },
                        onDragEnd = { endDrag(commit = true) },
                        onDragCancel = { endDrag(commit = false) },
                    )
                }
                Swipeable(
                    enabled = !dragging,
                    onRemove = { removed = removed + row.uid; player.removeFromQueue(row.index) },
                    modifier = Modifier
                        .then(if (dragging) Modifier else Modifier.animateItem(fadeInSpec = null, fadeOutSpec = null, placementSpec = spring(stiffness = 600f)))
                        .zIndex(if (dragging) 1f else 0f)
                        .graphicsLayer {
                            translationY = if (dragging) dragOffset else 0f
                            shadowElevation = if (dragging) 12f else 0f
                        },
                ) {
                    QueueItem(
                        row,
                        dim = false,
                        isCurrent = false,
                        handle = handle,
                        lifted = dragging,
                        onPlay = { player.playAt(row.index) },
                        onLongPress = { openActions(actionsFor(row, false)) },
                        moveUp = { if (row.position > currentPos + 1) player.moveInQueue(row.position, row.position - 1) },
                        moveDown = { if (row.position < queue.lastIndex) player.moveInQueue(row.position, row.position + 1) },
                    )
                }
            }
        }
    }
}

/** Swipe either way to remove (the row slides out over a red strip). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun Swipeable(onRemove: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true, content: @Composable () -> Unit) {
    val remove by rememberUpdatedState(onRemove)
    val state = rememberSwipeToDismissBoxState(confirmValueChange = { it == SwipeToDismissBoxValue.Settled || enabled })
    LaunchedEffect(state.currentValue) {
        if (state.currentValue != SwipeToDismissBoxValue.Settled) remove()
    }
    SwipeToDismissBox(
        state = state,
        modifier = modifier,
        gesturesEnabled = enabled,
        backgroundContent = {
            val toStart = state.dismissDirection == SwipeToDismissBoxValue.EndToStart
            Box(
                Modifier.fillMaxSize().background(MaterialTheme.colorScheme.error.copy(alpha = 0.25f)).padding(horizontal = 24.dp),
                contentAlignment = if (toStart) Alignment.CenterEnd else Alignment.CenterStart,
            ) {
                Icon(Icons.Rounded.Delete, contentDescription = null)
            }
        },
    ) { content() }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun QueueItem(
    row: QueueRow,
    dim: Boolean,
    isCurrent: Boolean,
    handle: Modifier?,
    onPlay: () -> Unit,
    onLongPress: () -> Unit,
    lifted: Boolean = false,
    moveUp: (() -> Unit)? = null,
    moveDown: (() -> Unit)? = null,
) {
    val palette = Palette.current
    val haptics = LocalHapticFeedback.current
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 60.dp)
            .background(
                when {
                    isCurrent -> palette.accent.copy(alpha = 0.14f)
                    lifted -> palette.surfaceHi
                    else -> palette.surface
                },
            )
            .combinedClickable(
                role = Role.Button,
                onClickLabel = if (isCurrent) null else "Play",
                onLongClickLabel = "More actions",
                onLongClick = { haptics.performHapticFeedback(HapticFeedbackType.LongPress); onLongPress() },
                onClick = onPlay,
            )
            .padding(start = 20.dp, end = if (handle != null) 4.dp else 20.dp, top = 6.dp, bottom = 6.dp)
            .semantics {
                selected = isCurrent
                val actions = buildList {
                    moveUp?.let { add(CustomAccessibilityAction("Move up") { it(); true }) }
                    moveDown?.let { add(CustomAccessibilityAction("Move down") { it(); true }) }
                }
                if (actions.isNotEmpty()) customActions = actions
            },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        val alpha = if (dim) 0.55f else 1f
        CoverImage(row.coverArt, Modifier.size(44.dp).graphicsLayer { this.alpha = alpha }, shape = RoundedCornerShape(6.dp))
        Column(Modifier.weight(1f)) {
            Text(
                row.title,
                color = (if (isCurrent) palette.accent else palette.text).copy(alpha = alpha),
                fontWeight = if (isCurrent) FontWeight.SemiBold else FontWeight.Normal,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(row.artist, style = MaterialTheme.typography.bodySmall, color = palette.textDim.copy(alpha = alpha), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (isCurrent) Icon(Icons.Rounded.GraphicEq, contentDescription = "Playing", tint = palette.accent, modifier = Modifier.size(20.dp))
        Text(formatTime(row.durationMs), style = MaterialTheme.typography.bodySmall, color = palette.textDim.copy(alpha = alpha))
        if (handle != null) {
            Box(handle.size(48.dp), contentAlignment = Alignment.Center) {
                Icon(Icons.Rounded.DragHandle, contentDescription = "Drag to reorder", tint = if (lifted) palette.accent else palette.textDim)
            }
        }
    }
}
