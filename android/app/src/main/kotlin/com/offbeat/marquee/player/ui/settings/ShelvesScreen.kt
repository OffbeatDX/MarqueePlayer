package com.offbeat.marquee.player.ui.settings

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.DragHandle
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import androidx.compose.foundation.clickable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.border
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.offbeat.marquee.core.library.ShelfId
import com.offbeat.marquee.core.settings.ShelfLayout
import com.offbeat.marquee.player.settings.UiSettings
import com.offbeat.marquee.player.ui.components.BackButton
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens
import kotlin.math.roundToInt
import com.offbeat.marquee.player.ui.components.rememberKeptScrollState

private val RowHeight = 64.dp

/**
 * Settings > Home > Edit shelves: drag a shelf by its handle to reorder, switch it to show or hide
 * it, Reset to defaults. Every change is saved at once, and Home follows it ([UiSettings.shelves]).
 * The screen reader gets "Move up" and "Move down" in place of dragging.
 */
@Composable
fun ShelvesScreen(onBack: () -> Unit) {
    val palette = Palette.current
    val saved by UiSettings.shelves.collectAsStateWithLifecycle()
    // The order while a drag is in progress (saved when it ends); otherwise what is saved.
    var dragLayout by remember { mutableStateOf<ShelfLayout?>(null) }
    val layout = dragLayout ?: saved
    var dragging by remember { mutableStateOf<ShelfId?>(null) }
    var dragOffset by remember { mutableFloatStateOf(0f) }
    val rowPx = with(LocalDensity.current) { RowHeight.toPx() }

    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
        Column(
            Modifier
                .widthIn(max = SettingsMaxWidth)
                .fillMaxSize()
                .verticalScroll(rememberKeptScrollState())
                .padding(top = LocalScreenTop.current + 4.dp, bottom = 32.dp),
        ) {
            Row(Modifier.padding(start = 8.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                BackButton(onBack)
                Text(
                    "Edit shelves",
                    style = MaterialTheme.typography.headlineSmall,
                    modifier = Modifier.padding(start = 12.dp).semantics { heading() },
                )
            }
            Text(
                "Home shows them in this order. Drag a handle to move a shelf; switch one off to hide it. Recently added is the large card wherever it sits.",
                style = MaterialTheme.typography.bodySmall,
                color = palette.textDim,
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
            )
            Column(
                Modifier
                    .padding(horizontal = 16.dp, vertical = 8.dp)
                    .fillMaxWidth()
                    .background(palette.surface, RoundedCornerShape(Tokens.RadiusCard))
                    .border(1.dp, palette.border, RoundedCornerShape(Tokens.RadiusCard)),
            ) {
                layout.order.forEachIndexed { index, id ->
                    key(id) {
                        val isDragged = dragging == id
                        val lift by animateFloatAsState(if (isDragged) 1f else 0f, label = "lift")
                        ShelfRow(
                            id = id,
                            shown = id !in layout.hidden,
                            canHide = layout.canHide(id),
                            index = index,
                            count = layout.order.size,
                            modifier = Modifier
                                .zIndex(if (isDragged) 1f else 0f)
                                .graphicsLayer { translationY = if (isDragged) dragOffset else 0f }
                                .shadow((lift * 8).dp, RoundedCornerShape(Tokens.RadiusCard))
                                .background(if (lift > 0f) palette.surfaceHi else palette.surface, RoundedCornerShape(Tokens.RadiusCard)),
                            handle = Modifier.pointerInput(id) {
                                detectDragGestures(
                                    onDragStart = {
                                        dragLayout = UiSettings.shelves.value
                                        dragging = id
                                        dragOffset = 0f
                                    },
                                    onDrag = { change, amount ->
                                        change.consume()
                                        val current = dragLayout ?: return@detectDragGestures
                                        dragOffset += amount.y
                                        val from = current.order.indexOf(id)
                                        val steps = (dragOffset / rowPx).roundToInt()
                                        val to = (from + steps).coerceIn(0, current.order.lastIndex)
                                        if (to != from) {
                                            dragLayout = current.move(id, to)
                                            dragOffset -= (to - from) * rowPx
                                        }
                                    },
                                    onDragEnd = {
                                        dragLayout?.let(UiSettings::setShelves)
                                        dragLayout = null
                                        dragging = null
                                        dragOffset = 0f
                                    },
                                    onDragCancel = {
                                        dragLayout?.let(UiSettings::setShelves)
                                        dragLayout = null
                                        dragging = null
                                        dragOffset = 0f
                                    },
                                )
                            },
                            onShown = { UiSettings.setShelves(saved.setShown(id, it)) },
                            onMove = { to -> UiSettings.setShelves(saved.move(id, to)) },
                        )
                    }
                }
            }
            OutlinedButton(
                onClick = { UiSettings.setShelves(ShelfLayout.DEFAULT) },
                enabled = !saved.isDefault,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp).height(48.dp),
            ) { Text("Reset to defaults") }
        }
    }
}

@Composable
private fun ShelfRow(
    id: ShelfId,
    shown: Boolean,
    canHide: Boolean,
    index: Int,
    count: Int,
    modifier: Modifier,
    handle: Modifier,
    onShown: (Boolean) -> Unit,
    onMove: (Int) -> Unit,
) {
    val palette = Palette.current
    Row(
        modifier
            .fillMaxWidth()
            .height(RowHeight)
            .semantics(mergeDescendants = true) {
                stateDescription = if (shown) "Shown, ${index + 1} of $count" else "Hidden"
                customActions = buildList {
                    if (index > 0) add(CustomAccessibilityAction("Move up") { onMove(index - 1); true })
                    if (index < count - 1) add(CustomAccessibilityAction("Move down") { onMove(index + 1); true })
                }
            },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(handle.size(56.dp, RowHeight), contentAlignment = Alignment.Center) {
            Icon(Icons.Rounded.DragHandle, contentDescription = null, tint = palette.textDim)
        }
        Column(
            Modifier
                .weight(1f)
                .clickable(enabled = shown.not() || canHide, role = Role.Switch) { onShown(!shown) }
                .padding(vertical = 8.dp),
        ) {
            Text(id.title, style = MaterialTheme.typography.bodyLarge, color = if (shown) palette.text else palette.textDim)
            Text(
                when {
                    id.isHero -> "The large card"
                    !shown -> "Hidden"
                    else -> "A row of albums"
                },
                style = MaterialTheme.typography.bodySmall,
                color = palette.textFaint,
            )
        }
        Switch(
            checked = shown,
            onCheckedChange = onShown,
            enabled = !shown || canHide,
            colors = SwitchDefaults.colors(
                checkedThumbColor = palette.onAccent,
                checkedTrackColor = palette.accent,
                uncheckedThumbColor = palette.textDim,
                uncheckedTrackColor = palette.surfaceHi,
                uncheckedBorderColor = palette.textFaint,
            ),
            modifier = Modifier.padding(end = 16.dp),
        )
    }
}
