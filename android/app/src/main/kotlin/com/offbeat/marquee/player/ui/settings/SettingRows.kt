package com.offbeat.marquee.player.ui.settings

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.runtime.remember
import androidx.compose.ui.unit.dp
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens

// The desktop's settings pieces (widgets.slint: SettingCard, SettingRow, SwitchRow, Segmented),
// phone-sized: a card per section; each row its name, one short line saying what it does, and its
// control at the right edge. Choices with more options than fit beside the name sit under it.

/** A section: its title over a card of rows, hairlines between them. */
@Composable
fun SettingCard(title: String, modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val palette = Palette.current
    Column(modifier.fillMaxWidth()) {
        Text(
            title,
            style = MaterialTheme.typography.titleMedium,
            color = palette.text,
            modifier = Modifier.padding(start = 4.dp, bottom = 8.dp).semantics { heading() },
        )
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(Tokens.RadiusCard))
                .background(palette.surface)
                .border(BorderStroke(1.dp, palette.border), RoundedCornerShape(Tokens.RadiusCard)),
            content = content,
        )
    }
}

/** The hairline between two rows of a card. */
@Composable
fun RowDivider() {
    HorizontalDivider(Modifier.padding(start = 16.dp), thickness = 1.dp, color = Palette.current.border)
}

/**
 * One setting: [name], [hint] under it, [trailing] at the right edge, [below] under both (a
 * segmented control too wide to sit beside the name). With [onClick] the whole row acts.
 */
@Composable
fun SettingRow(
    name: String,
    hint: String?,
    modifier: Modifier = Modifier,
    onClick: (() -> Unit)? = null,
    role: Role? = null,
    enabled: Boolean = true,
    hintColor: Color = Palette.current.textDim,
    trailing: (@Composable () -> Unit)? = null,
    below: (@Composable () -> Unit)? = null,
) {
    val palette = Palette.current
    val clickable = if (onClick != null) Modifier.clickable(enabled = enabled, role = role, onClick = onClick) else Modifier
    Column(
        modifier
            .fillMaxWidth()
            .then(clickable)
            .heightIn(min = 64.dp)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(end = if (trailing != null) 12.dp else 0.dp)) {
                Text(name, style = MaterialTheme.typography.bodyLarge, color = if (enabled) palette.text else palette.textDim)
                if (!hint.isNullOrEmpty()) {
                    Text(hint, style = MaterialTheme.typography.bodySmall, color = hintColor, modifier = Modifier.padding(top = 2.dp))
                }
            }
            trailing?.invoke()
        }
        if (below != null) {
            Box(Modifier.padding(top = 10.dp)) { below() }
        }
    }
}

/** A two-state setting: the whole row toggles. */
@Composable
fun SwitchSettingRow(name: String, hint: String?, checked: Boolean, onToggle: (Boolean) -> Unit, enabled: Boolean = true) {
    val palette = Palette.current
    SettingRow(
        name = name,
        hint = hint,
        onClick = { onToggle(!checked) },
        role = Role.Switch,
        enabled = enabled,
        trailing = {
            Switch(
                checked = checked,
                onCheckedChange = null,
                enabled = enabled,
                colors = SwitchDefaults.colors(
                    checkedThumbColor = palette.onAccent,
                    checkedTrackColor = palette.accent,
                    uncheckedThumbColor = palette.textDim,
                    uncheckedTrackColor = palette.surfaceHi,
                    uncheckedBorderColor = palette.textFaint,
                ),
            )
        },
    )
}

/** A row that opens a page: its state as the hint, a chevron at the edge. */
@Composable
fun NavSettingRow(name: String, hint: String?, onClick: () -> Unit, hintColor: Color = Palette.current.textDim) {
    SettingRow(
        name = name,
        hint = hint,
        onClick = onClick,
        role = Role.Button,
        hintColor = hintColor,
        trailing = { Icon(Icons.AutoMirrored.Rounded.KeyboardArrowRight, contentDescription = null, tint = Palette.current.textDim) },
    )
}

/** A small choice with every option visible (the desktop's Segmented), full width under the name. */
@Composable
fun <T> SegmentedSettingRow(
    name: String,
    hint: String?,
    options: List<T>,
    selected: T,
    label: (T) -> String,
    onSelect: (T) -> Unit,
    describe: (T) -> String = label,
) {
    SettingRow(name = name, hint = hint, below = { Segmented(name, options, selected, label, onSelect, describe) })
}

@Composable
fun <T> Segmented(
    name: String,
    options: List<T>,
    selected: T,
    label: (T) -> String,
    onSelect: (T) -> Unit,
    describe: (T) -> String = label,
) {
    val palette = Palette.current
    val shape = RoundedCornerShape(24.dp)
    val base = MaterialTheme.typography.labelLarge
    val measurer = rememberTextMeasurer()
    // Segments share the width by their labels' measured width (plus 12 dp each side), so
    // "Original" fits beside "320" at any font size; when even that can't fit (font size 2.0 on a
    // 360 dp phone clipped it to "Origina"), the labels shrink just enough.
    val widths = remember(options, base, measurer) { options.map { measurer.measure(label(it), base, maxLines = 1).size.width } }
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val density = LocalDensity.current
        val pad = with(density) { 24.dp.toPx() }
        val room = with(density) { (maxWidth - 1.dp * (options.size - 1)).toPx() }
        val scale = (room / (widths.sum() + pad * options.size)).coerceAtMost(1f)
        val style = if (scale < 1f) base.copy(fontSize = base.fontSize * scale) else base
        Row(
            Modifier
                .fillMaxWidth()
                .height(48.dp)
                .clip(shape)
                .border(1.dp, palette.textFaint.copy(alpha = 0.45f), shape)
                .selectableGroup()
                .semantics { contentDescription = name },
        ) {
            options.forEachIndexed { i, option ->
                val chosen = option == selected
                if (i > 0) Box(Modifier.fillMaxHeight().width(1.dp).background(palette.textFaint.copy(alpha = 0.45f)))
                Box(
                    Modifier
                        .weight(widths[i] + pad)
                        .fillMaxHeight()
                        .background(if (chosen) palette.accent.copy(alpha = 0.18f) else Color.Transparent)
                        .selectable(selected = chosen, role = Role.RadioButton) { if (!chosen) onSelect(option) }
                        .semantics { contentDescription = describe(option) },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        label(option),
                        maxLines = 1,
                        softWrap = false,
                        style = style,
                        color = if (chosen) palette.accent else palette.text,
                    )
                }
            }
        }
    }
}

/** Colour swatches in a row; the chosen one ringed and ticked. */
@Composable
fun SwatchPicker(colors: List<Color>, names: List<String>, selected: Int, onSelect: (Int) -> Unit) {
    val palette = Palette.current
    // Each swatch takes an equal share of the row (at least 40 dp wide on a 360 dp phone) and the
    // full 48 dp height, so the touch target stays large while eight fit.
    Row(Modifier.fillMaxWidth()) {
        colors.forEachIndexed { i, c ->
            val chosen = i == selected
            Box(
                Modifier
                    .weight(1f)
                    .height(48.dp)
                    .clip(RoundedCornerShape(24.dp))
                    .clickable(role = Role.RadioButton, onClickLabel = "Use ${names[i]}") { onSelect(i) }
                    .semantics {
                        contentDescription = names[i]
                        this.selected = chosen
                    },
                contentAlignment = Alignment.Center,
            ) {
                Box(
                    Modifier
                        .size(if (chosen) 34.dp else 30.dp)
                        .then(if (chosen) Modifier.border(2.dp, palette.text, CircleShape) else Modifier)
                        .padding(if (chosen) 3.dp else 0.dp)
                        .clip(CircleShape)
                        .background(c),
                    contentAlignment = Alignment.Center,
                ) {
                    if (chosen) Icon(Icons.Rounded.Check, contentDescription = null, tint = Color(0xFF15131A), modifier = Modifier.size(16.dp))
                }
            }
        }
    }
}

/** A quiet line of information in a card (no control). */
@Composable
fun InfoRow(name: String, value: String) {
    val palette = Palette.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(name, style = MaterialTheme.typography.bodyLarge, color = palette.text, modifier = Modifier.weight(1f))
        Text(value, style = MaterialTheme.typography.bodyMedium, color = palette.textDim, modifier = Modifier.padding(start = 12.dp))
    }
}

/** Space between sections. */
val SectionGap = 24.dp
val SettingsMaxWidth = 640.dp
