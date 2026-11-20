package com.offbeat.marquee.core.eq

import kotlin.math.abs
import kotlin.math.pow

/**
 * Equalizer APO's configuration text, the form AutoEq's `ParametricEQ.txt` and `FixedBandEQ.txt`
 * are, read as the desktop's `eq/apo.rs` reads it (Equalizer APO's own rules), with a smaller scope:
 *
 * - `Preamp: <dB> dB`; several add up.
 * - `Filter[ n]: ON <type> ...`; the parameters anywhere after the type: `Fc <Hz> Hz`, `Gain <dB> dB`,
 *   `Q <q>`, `BW Oct <octaves>`, and a shelf's slope as `<x> dB` right after its type. Commas are
 *   decimal points; a frequency like "1.000" (a point four from its end) is a thousands separator.
 * - Types: `PK`, `PEQ`, `Modal`; `LP`, `LPQ`, `HP`, `HPQ`, `BP` (default Q 1/sqrt(2)); `NO` (default
 *   Q 30); `AP`; `LS`, `HS`, `LSC`, `HSC`. A shelf whose type doesn't end in C is given by its corner
 *   frequency and moved to its midpoint by Equalizer APO's factor 10^(|gain| / 80 / S); a shelf with
 *   neither a Q nor a slope is S 0.9.
 * - `OFF` filters and REW's `None` slots play nothing (said in a note). Filter types it can't play
 *   (`IIR`, and names Equalizer APO doesn't have) are skipped with a note, as are `GraphicEQ:` curves,
 *   `Copy:`, `Delay:` and `LoudnessCorrection:`, and anything for one channel or one device only.
 * - A filter missing what it needs (no frequency, no gain, no Q for a peak), a preamp that isn't a
 *   number, `Include:`, `Convolution:` and expressions (`If:`, `Eval:`, inline `` ` ``) make the
 *   import fail with the line named: the profile would not play as written.
 */
object Apo {
    /** Imported, with what was read in a particular way or left out ("Line 4: an OFF filter …"). */
    data class Profile(val bands: List<Band>, val preampDb: Double?, val notes: List<String>)

    sealed interface Result {
        data class Ok(val profile: Profile) : Result
        /** [line] is 1-based, or null when the text as a whole is the problem. */
        data class Error(val line: Int?, val message: String) : Result
    }

    /** At most this much text is read (an AutoEq file is under 1 KB). */
    const val MAX_CHARS = 256 * 1024

    /** A number is read from at most this many characters (keeps a run of digits from being slow). */
    private const val MAX_NUMBER = 64

    fun read(text: String): Result {
        if (text.length > MAX_CHARS) return Result.Error(null, "This is too long for an equalizer profile (over 256 KB).")
        val bands = ArrayList<Band>()
        val notes = ArrayList<String>()
        var preamp: Double? = null
        var understood = false
        var offFilters = 0
        var channelElsewhere: String? = null
        var deviceElsewhere: String? = null
        var graphicSkipped = false
        val lines = text.split("\r\n", "\n", "\r")
        for ((i, raw) in lines.withIndex()) {
            val no = i + 1
            val line = raw.trim().trimStart('﻿')
            if (line.isEmpty() || line.startsWith("#")) continue
            val colon = line.indexOf(':')
            if (colon < 0) continue
            val cmd = line.substring(0, colon).trim()
            val args = line.substring(colon + 1)
            val lc = cmd.lowercase()
            val elsewhere = channelElsewhere ?: deviceElsewhere
            when {
                lc == "preamp" -> {
                    understood = true
                    val v = floatPrefix(args.replace(',', '.').trim())
                        ?: return Result.Error(no, "Line $no: the preamp is not a number (“${line}”).")
                    if (elsewhere != null) notes += "Line $no: a preamp for $elsewhere: left out (both channels are treated alike)."
                    else preamp = (preamp ?: 0.0) + v
                }
                lc == "channel" -> { understood = true; channelElsewhere = channelScope(args) }
                lc == "device" -> {
                    understood = true
                    deviceElsewhere = if (args.split(';').any { it.trim().equals("all", ignoreCase = true) }) null else "the device “${args.trim()}” only"
                }
                lc == "graphiceq" -> { understood = true; graphicSkipped = true; notes += "Line $no: a GraphicEQ curve is not read here: left out (import AutoEq’s ParametricEQ.txt instead)." }
                lc == "include" -> return Result.Error(no, "Line $no includes another file (“${args.trim()}”), which is not in this text: import that file instead.")
                lc == "convolution" -> return Result.Error(no, "Line $no is a convolution with an impulse response file, which this equalizer can’t play: use AutoEq’s ParametricEQ.txt.")
                lc in setOf("if", "elseif", "else", "endif", "eval") -> return Result.Error(no, "Line $no is an expression (“$cmd:”), worked out by Equalizer APO as it runs; this equalizer can’t follow it.")
                lc == "copy" -> { understood = true; notes += "Line $no: channel mixing (“Copy:”) has no equivalent here: left out." }
                lc == "delay" -> { understood = true; notes += "Line $no: a delay has no equivalent here: left out." }
                lc == "loudnesscorrection" -> { understood = true; notes += "Line $no: Equalizer APO’s loudness correction has no equivalent here: left out." }
                lc == "stage" -> understood = true
                lc.startsWith("filter") -> {
                    understood = true
                    if ('`' in args) return Result.Error(no, "Line $no has an inline expression (`…`), worked out by Equalizer APO as it runs; this equalizer can’t follow it.")
                    when (val f = filterLine(no, line, args)) {
                        is FilterLine.Ok -> {
                            if (elsewhere != null) notes += "Line $no: a filter for $elsewhere: left out (both channels are treated alike)."
                            else { bands += f.band; notes += f.notes }
                        }
                        is FilterLine.Off -> { offFilters++; notes += "Line $no: an OFF filter, which plays nothing." }
                        FilterLine.None -> Unit
                        is FilterLine.Skip -> notes += f.why
                        is FilterLine.Broken -> return Result.Error(no, f.why)
                    }
                }
                // Room EQ Wizard's header lines and anything else Equalizer APO ignores.
                else -> Unit
            }
        }
        if (!understood) return Result.Error(null, "This text has no equalizer settings in it (no “Preamp:” or “Filter:” lines).")
        if (bands.isEmpty()) {
            val why = when {
                offFilters > 0 -> "All $offFilters of its filters are OFF: there is nothing to play."
                graphicSkipped -> "It holds only a GraphicEQ curve, which isn’t read here: import AutoEq’s ParametricEQ.txt instead."
                else -> "No filters in this text."
            }
            return Result.Error(null, why)
        }
        if (bands.size > MAX_BANDS) return Result.Error(null, "It has ${bands.size} filters; this equalizer plays up to $MAX_BANDS.")
        val held = preamp?.coerceIn(PREAMP_MIN, PREAMP_MAX)
        if (held != preamp) notes += "The preamp is held within the equalizer’s range (−30 dB to +12 dB)."
        return Result.Ok(Profile(bands, held, notes))
    }

    /** A profile name from a file's: AutoEq's "Sennheiser HD 650 ParametricEQ.txt" is "Sennheiser HD 650". */
    fun nameFromFile(fileName: String): String {
        var t = fileName.trim()
        if (t.lowercase().endsWith(".txt")) t = t.dropLast(4)
        t = t.trim()
        val lower = t.lowercase()
        for (suffix in listOf("parametriceq", "fixedbandeq", "graphiceq")) {
            if (lower.endsWith(suffix)) return t.dropLast(suffix.length).trimEnd(' ', '_', '-').trim().ifEmpty { t }
        }
        return t
    }

    private sealed interface FilterLine {
        class Ok(val band: Band, val notes: List<String>) : FilterLine
        class Off : FilterLine
        data object None : FilterLine
        class Skip(val why: String) : FilterLine
        class Broken(val why: String) : FilterLine
    }

    private fun kindOf(ty: String): Kind? = when (ty) {
        "PK", "PEQ", "MODAL" -> Kind.Peak
        "LP", "LPQ" -> Kind.LowPass
        "HP", "HPQ" -> Kind.HighPass
        "BP" -> Kind.BandPass
        "LS", "LSC" -> Kind.LowShelf
        "HS", "HSC" -> Kind.HighShelf
        "NO" -> Kind.Notch
        "AP" -> Kind.AllPass
        else -> null
    }

    private fun filterLine(no: Int, raw: String, argsIn: String): FilterLine {
        val params = argsIn.replace(',', '.')
        val t = params.trimStart()
        val sp = t.indexOfFirst { it.isWhitespace() }.let { if (it < 0) t.length else it }
        val state = t.substring(0, sp)
        when (state.uppercase()) {
            "ON" -> Unit
            "OFF" -> return FilterLine.Off()
            else -> return FilterLine.Broken("Line $no: a filter starts with ON or OFF: “$raw”.")
        }
        var rest = t.substring(sp).trimStart()
        val tyEnd = rest.indexOfFirst { !(it in 'a'..'z' || it in 'A'..'Z') }.let { if (it < 0) rest.length else it }
        val tyRaw = rest.substring(0, tyEnd)
        val ty = tyRaw.uppercase()
        rest = rest.substring(tyEnd)
        if (ty.isEmpty()) return FilterLine.Broken("Line $no: no filter type: “$raw”.")
        if (ty == "NONE") return FilterLine.None
        if (ty == "IIR") return FilterLine.Skip("Line $no: a filter given by its coefficients (IIR), which are for one sample rate: left out.")
        val kind = kindOf(ty) ?: return FilterLine.Skip("Line $no: “$tyRaw” is not a filter type this equalizer plays: left out.")
        val notes = ArrayList<String>()
        val (freq, thousands) = paramFreq(rest)
            ?: return FilterLine.Broken("Line $no: the filter has no frequency (“Fc … Hz”): “$raw”.")
        if (thousands) notes += "Line $no: the frequency is read as ${freq.toInt()} Hz, the point a thousands separator, as Equalizer APO reads it."
        if (freq <= 0.0) return FilterLine.Broken("Line $no: the frequency is not above 0 Hz: “$raw”.")
        val gain = if (kind.hasGain) {
            paramGain(rest) ?: return FilterLine.Broken("Line $no: the filter has no gain (“Gain … dB”): “$raw”.")
        } else 0.0
        // As Equalizer APO: a Q, then a bandwidth (not for shelves), then a shelf's slope.
        var value = paramQ(rest) ?: 0.0
        var width = Width.Q
        if (!kind.isShelf) paramBw(rest)?.let { value = it; width = Width.Octaves }
        if (kind.isShelf) paramSlope(rest)?.let { value = it; width = Width.Slope }
        var corner = false
        if (value == 0.0) {
            when (kind) {
                Kind.Peak, Kind.AllPass -> return FilterLine.Broken("Line $no: the filter has no Q or bandwidth: “$raw”.")
                Kind.LowPass, Kind.HighPass, Kind.BandPass -> value = 0.7071067811865476
                Kind.LowShelf, Kind.HighShelf -> { value = 0.9; width = Width.Slope }
                Kind.Notch -> value = 30.0
            }
        } else if (kind.isShelf) {
            if (width == Width.Slope) value /= 12.0
            corner = !ty.endsWith("C")
        }
        if (value < 0.0) return FilterLine.Broken("Line $no: a negative width: “$raw”.")
        var f = freq
        if (corner) {
            val a = 10.0.pow(gain / 40.0)
            val s = if (width == Width.Q) 1.0 / ((1.0 / (value * value) - 2.0) / (a + 1.0 / a) + 1.0) else value
            val factor = 10.0.pow(abs(gain) / 80.0 / s)
            if (!factor.isFinite() || factor <= 0.0 || s <= 0.0) return FilterLine.Broken("Line $no: this shelf’s corner frequency and Q do not give a shelf: “$raw”.")
            f = if (kind == Kind.LowShelf) freq * factor else freq / factor
            notes += "Line $no: a shelf given by its corner frequency (${hz(freq)}), placed at its midpoint, ${hz(f)}, as Equalizer APO places it."
        }
        if (width == Width.Slope) {
            val a = 10.0.pow(gain / 40.0)
            if ((a + 1.0 / a) * (1.0 / value - 1.0) + 2.0 <= 0.0) return FilterLine.Broken("Line $no: this slope is steeper than a shelf of this gain can be: “$raw”.")
        }
        val band = Band.of(kind, f, gain, value, width)
        fun off(x: Double, y: Double) = abs(x - y) > 1e-3 * maxOf(abs(x), 1.0)
        if (off(f, band.freq) || (kind.hasGain && off(gain, band.gainDb)) || off(value, band.q)) {
            notes += "Line $no: held within the equalizer’s range (10 Hz to 24 kHz, ±24 dB)."
        }
        return FilterLine.Ok(band, notes)
    }

    private fun channelScope(args: String): String? {
        val words = args.split(Regex("\\s+")).filter { it.isNotEmpty() }.map { it.uppercase() }
        if ("ALL" in words) return null
        val left = words.any { it == "L" || it == "1" }
        val right = words.any { it == "R" || it == "2" }
        return when {
            left && right -> null
            left -> "the left channel only"
            right -> "the right channel only"
            else -> "channels a stereo output does not have (${args.trim()})"
        }
    }

    /** The longest number at the start of [s], as C's wcstod reads it ("1.2.3" is 1.2), from its first [MAX_NUMBER] characters. */
    internal fun floatPrefix(s: String): Double? {
        var end = 0
        var best: Double? = null
        while (end < s.length && end < MAX_NUMBER && (s[end].isDigit() || s[end] in "+-.eE")) {
            end++
            s.substring(0, end).toDoubleOrNull()?.takeIf { it.isFinite() }?.let { best = it }
        }
        return best
    }

    /** Case-insensitive search for [word] preceded by whitespace; the index just after it. */
    private fun findWord(hay: String, word: String): Int? {
        val lower = hay.lowercase()
        var from = 0
        while (true) {
            val at = lower.indexOf(word, from)
            if (at < 0) return null
            if (at > 0 && lower[at - 1].isWhitespace()) return at + word.length
            from = at + word.length
        }
    }

    private fun numberRun(s: String, nbsp: Boolean): Pair<String, String> {
        val t = s.trimStart()
        var end = 0
        while (end < t.length && (t[end].isDigit() || t[end] in "+-.eE" || (nbsp && t[end] == ' '))) end++
        return t.substring(0, end) to t.substring(end)
    }

    private fun paramFreq(rest: String): Pair<Double, Boolean>? {
        val at = findWord(rest, "fc") ?: return null
        val (run, after) = numberRun(rest.substring(at), nbsp = true)
        val unit = after.filterNot { it.isWhitespace() }.take(2).lowercase()
        if (run.isEmpty() || unit != "hz") return null
        val s = run.filter { it != ' ' }
        var v = floatPrefix(s) ?: return null
        val thousands = s.length >= 5 && 'e' !in s && 'E' !in s && s[s.length - 4] == '.'
        if (thousands) v *= 1000.0
        return v to thousands
    }

    private fun paramGain(rest: String): Double? {
        val at = findWord(rest, "gain") ?: return null
        val (run, after) = numberRun(rest.substring(at), nbsp = false)
        if (!after.trimStart().lowercase().startsWith("db")) return null
        return floatPrefix(run)
    }

    private fun paramQ(rest: String): Double? {
        val at = findWord(rest, "q") ?: return null
        val (run, _) = numberRun(rest.substring(at), nbsp = false)
        if (run.isEmpty()) return null
        return floatPrefix(run) ?: 0.0
    }

    private fun paramBw(rest: String): Double? {
        val at = findWord(rest, "bw") ?: return null
        val after = rest.substring(at)
        if (after.isEmpty() || !after[0].isWhitespace()) return null
        val a = after.trimStart()
        if (!a.lowercase().startsWith("oct")) return null
        val (run, _) = numberRun(a.substring(3), nbsp = false)
        if (run.isEmpty()) return null
        return floatPrefix(run) ?: 0.0
    }

    private fun paramSlope(rest: String): Double? {
        val (run, after) = numberRun(rest, nbsp = false)
        if (run.isEmpty() || !after.trimStart().lowercase().startsWith("db")) return null
        return floatPrefix(run)
    }

    private fun hz(f: Double): String = if (f >= 1000.0) "%.2f kHz".format(java.util.Locale.ROOT, f / 1000.0).replace(".00 ", " ") else "%.0f Hz".format(java.util.Locale.ROOT, f)

}
