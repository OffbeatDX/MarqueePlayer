package com.offbeat.marquee.player.ui.eq

import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.offbeat.marquee.core.eq.Band
import com.offbeat.marquee.core.eq.responseCurve
import com.offbeat.marquee.player.ui.theme.Palette
import kotlin.math.ln
import kotlin.math.pow
import kotlin.math.roundToInt

private const val F_LO = 20.0
private const val F_HI = 20_000.0
private const val DB = 12.0
/** Points of the curve across the width (log-spaced). */
private const val POINTS = 160

private val CURVE_FREQS = DoubleArray(POINTS) { F_LO * (F_HI / F_LO).pow(it / (POINTS - 1.0)) }

/** Where a frequency sits across the width (0 to 1, log scale 20 Hz to 20 kHz). */
private fun xOf(f: Double) = (ln(f / F_LO) / ln(F_HI / F_LO)).toFloat()

/**
 * The equalizer's curve across the width: the bands' combined response (at 48 kHz) on a log frequency
 * axis, 20 Hz to 20 kHz, with a ±12 dB grid. Drawn from the bands' own math, the same the audio thread runs.
 */
@Composable
fun EqCurve(bands: List<Band>, on: Boolean, modifier: Modifier = Modifier) {
    val palette = Palette.current
    val values = remember(bands) { responseCurve(bands, 48_000, CURVE_FREQS) }
    val measurer = rememberTextMeasurer()
    val labelStyle = TextStyle(color = palette.textFaint, fontSize = 10.sp)
    val lo = values.minOrNull() ?: 0.0
    val hi = values.maxOrNull() ?: 0.0
    val description = if (hi - lo < 0.05) "Response curve: flat" else "Response curve, from ${fmt(lo)} to ${fmt(hi)} dB"
    val line = if (on) palette.accent else palette.textDim
    Canvas(modifier.semantics { contentDescription = description }) {
        val w = size.width
        val h = size.height
        val pad = 4.dp.toPx()
        val plotH = h - 2 * pad
        fun y(db: Double) = pad + ((DB - db.coerceIn(-DB - 1, DB + 1)) / (2 * DB) * plotH).toFloat()
        // Grid: every 6 dB (0 brighter), and the decades.
        for (db in listOf(-12.0, -6.0, 0.0, 6.0, 12.0)) {
            drawLine(if (db == 0.0) palette.textFaint.copy(alpha = 0.5f) else palette.border, Offset(0f, y(db)), Offset(w, y(db)), strokeWidth = 1.dp.toPx())
        }
        for (f in listOf(50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0)) {
            val x = xOf(f) * w
            drawLine(palette.border, Offset(x, pad), Offset(x, h - pad), strokeWidth = 1.dp.toPx())
        }
        for ((f, t) in listOf(100.0 to "100", 1000.0 to "1k", 10_000.0 to "10k")) {
            drawText(measurer, t, Offset(xOf(f) * w + 3.dp.toPx(), h - pad - 14.dp.toPx()), labelStyle)
        }
        for ((db, t) in listOf(12.0 to "+12", -12.0 to "−12")) {
            drawText(measurer, t, Offset(3.dp.toPx(), if (db > 0) y(db) + 1.dp.toPx() else y(db) - 14.dp.toPx()), labelStyle)
        }
        val path = Path()
        val fill = Path()
        val zero = y(0.0)
        for (i in 0 until POINTS) {
            val x = xOf(CURVE_FREQS[i]) * w
            val yy = y(values[i])
            if (i == 0) { path.moveTo(x, yy); fill.moveTo(x, zero); fill.lineTo(x, yy) } else { path.lineTo(x, yy); fill.lineTo(x, yy) }
        }
        fill.lineTo(w, zero)
        fill.close()
        drawPath(fill, line.copy(alpha = if (on) 0.18f else 0.08f))
        drawPath(path, line, style = Stroke(width = 2.dp.toPx()))
    }
}

private fun fmt(db: Double): String {
    val r = (db * 10).roundToInt() / 10.0
    return if (r > 0) "+$r" else if (r < 0) "−${-r}" else "0"
}

/** "+3.5", "−2", "0" (a true minus sign). */
fun dbLabel(db: Double, decimals: Boolean = true): String {
    val r = (db * 10).roundToInt() / 10.0
    val body = if (decimals || r != r.roundToInt().toDouble()) "%.1f".format(java.util.Locale.ROOT, kotlin.math.abs(r)).removeSuffix(".0") else "${kotlin.math.abs(r).roundToInt()}"
    return when {
        r > 0 -> "+$body"
        r < 0 -> "−$body"
        else -> "0"
    }
}
