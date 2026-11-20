package com.offbeat.marquee.player.ui.eq

import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextStyle
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.offbeat.marquee.core.eq.Graphic
import com.offbeat.marquee.player.ui.theme.Palette
import kotlin.math.roundToInt

/** A slider moves in half-dB steps. */
private const val STEP = 0.5

/**
 * One band of the graphic equalizer: a vertical slider, +12 dB at the top, −12 at the bottom. Drag
 * anywhere in its column (it moves with the finger, relative to where it was), double-tap to reset it
 * to 0 dB. To TalkBack it is an adjustable range ("1 kilohertz, +3 dB") with a Reset action.
 */
@Composable
fun BandSlider(
    label: String,
    spoken: String,
    value: Double,
    onChange: (Double) -> Unit,
    onDone: () -> Unit,
    on: Boolean,
    modifier: Modifier = Modifier,
) {
    val palette = Palette.current
    val current by rememberUpdatedState(value)
    val change by rememberUpdatedState(onChange)
    val done by rememberUpdatedState(onDone)
    val max = Graphic.MAX_DB
    Column(modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        Text(
            dbLabel(value),
            style = bandLabelStyle(),
            color = if (value != 0.0) palette.text else palette.textDim,
            textAlign = TextAlign.Center,
            maxLines = 1,
            modifier = Modifier.fillMaxWidth(),
        )
        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .semantics {
                    contentDescription = spoken
                    stateDescription = "${dbLabel(value)} dB"
                    progressBarRangeInfo = ProgressBarRangeInfo(value.toFloat(), -max.toFloat()..max.toFloat(), steps = (2 * max / STEP).toInt() - 1)
                    setProgress { v -> change(snap(v.toDouble())); done(); true }
                    customActions = listOf(CustomAccessibilityAction("Reset to 0 dB") { change(0.0); done(); true })
                }
                .pointerInput(Unit) { detectTapGestures(onDoubleTap = { change(0.0); done() }) }
                .pointerInput(Unit) {
                    var raw = 0.0
                    detectVerticalDragGestures(
                        onDragStart = { raw = current },
                        onDragEnd = { done() },
                        onDragCancel = { done() },
                    ) { ev, dy ->
                        ev.consume()
                        val usable = (size.height - 24.dp.toPx()).coerceAtLeast(1f)
                        raw = (raw - dy / usable * 2 * max).coerceIn(-max, max)
                        val v = snap(raw)
                        if (v != current) change(v)
                    }
                }
                .drawBehind {
                    val cx = size.width / 2
                    val inset = 12.dp.toPx()
                    val top = inset
                    val bottom = size.height - inset
                    fun y(db: Double) = (top + (max - db) / (2 * max) * (bottom - top)).toFloat()
                    val track = 4.dp.toPx()
                    drawLine(palette.surfaceHi, Offset(cx, top), Offset(cx, bottom), strokeWidth = track, cap = StrokeCap.Round)
                    // Ticks at 0 and ±6.
                    for (db in listOf(-6.0, 0.0, 6.0)) {
                        val w = if (db == 0.0) 9.dp.toPx() else 5.dp.toPx()
                        drawLine(palette.textFaint.copy(alpha = 0.6f), Offset(cx - w, y(db)), Offset(cx + w, y(db)), strokeWidth = 1.dp.toPx())
                    }
                    val color = if (on) palette.accent else palette.textDim
                    if (value != 0.0) drawLine(color, Offset(cx, y(0.0)), Offset(cx, y(value)), strokeWidth = track, cap = StrokeCap.Round)
                    drawCircle(color, radius = 9.dp.toPx(), center = Offset(cx, y(value)))
                },
        )
        Text(
            label,
            style = bandLabelStyle(),
            color = palette.textDim,
            textAlign = TextAlign.Center,
            maxLines = 1,
            modifier = Modifier.fillMaxWidth().height(18.dp).padding(top = 2.dp),
        )
    }
}

/**
 * The value and frequency labels grow with the font size only up to 1.2x: ten of them share the
 * width, and at 2.0 "+5.4" no longer fit its column ("+5.+4.+1."). TalkBack reads the values from
 * the slider's semantics, at any size.
 */
@Composable
private fun bandLabelStyle(): TextStyle {
    val base = MaterialTheme.typography.labelSmall
    val scale = LocalDensity.current.fontScale
    return if (scale <= 1.2f) base else base.copy(fontSize = base.fontSize * (1.2f / scale))
}

private fun snap(v: Double): Double = ((v / STEP).roundToInt() * STEP).coerceIn(-Graphic.MAX_DB, Graphic.MAX_DB)
