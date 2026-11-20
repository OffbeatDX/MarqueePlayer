package com.offbeat.marquee.core.playback

import com.offbeat.marquee.core.subsonic.ReplayGain
import kotlin.math.min
import kotlin.math.pow

enum class ReplayGainMode {
    Off, Track, Album;

    companion object {
        val DEFAULT = Track
        fun fromName(name: String?): ReplayGainMode = entries.firstOrNull { it.name == name } ?: DEFAULT
    }
}

/** The little the album-in-order rule needs to know about a queue entry. */
data class QueueEntry(val albumId: String?, val discNumber: Int?, val track: Int?)

/**
 * ReplayGain as player volume. The player can only attenuate (volume is 0..1), so a positive gain
 * is capped at 0 dB, and the result is also capped so the peak never goes past full scale.
 */
object ReplayGainMath {
    fun dbToLinear(db: Double): Double = 10.0.pow(db / 20.0)

    /** The player volume for a gain in dB and its peak (linear, 1.0 = full scale). No gain → 1.0. */
    fun volume(gainDb: Double?, peak: Double?): Float {
        if (gainDb == null || gainDb.isNaN()) return 1f
        var v = dbToLinear(gainDb)
        if (peak != null && peak > 0.0) v = min(v, 1.0 / peak)
        return v.coerceIn(0.0, 1.0).toFloat()
    }

    /**
     * The gain and peak to use for a song.
     *
     * - Off: nothing.
     * - Track: the track gain; nothing when the song has none.
     * - Album: the album gain only while the queue plays that album in order ([albumInOrder]);
     *   otherwise, or when there is no album gain, the track gain, as in Track mode.
     *
     * A song without a replayGain object gets nothing (volume 1.0).
     */
    fun select(mode: ReplayGainMode, rg: ReplayGain?, albumInOrder: Boolean): Pair<Double, Double?>? {
        if (rg == null || mode == ReplayGainMode.Off) return null
        if (mode == ReplayGainMode.Album && albumInOrder && rg.albumGain != null) {
            return rg.albumGain to (rg.albumPeak ?: rg.trackPeak)
        }
        return rg.trackGain?.let { it to rg.trackPeak }
    }

    fun volumeFor(mode: ReplayGainMode, rg: ReplayGain?, albumInOrder: Boolean): Float =
        select(mode, rg, albumInOrder)?.let { (gain, peak) -> volume(gain, peak) } ?: 1f

    /**
     * Whether the entry at [index] (in play order) is part of its album played in order: shuffle is
     * off and the entry next to it in play order (before or after) is from the same album, with the
     * one before earlier in the album and the one after later. A lone track from an album in a
     * mixed queue is not, so it gets its track gain.
     */
    fun albumInOrder(playOrder: List<QueueEntry>, index: Int, shuffle: Boolean): Boolean {
        if (shuffle) return false
        val e = playOrder.getOrNull(index) ?: return false
        val album = e.albumId ?: return false
        val prev = playOrder.getOrNull(index - 1)
        val next = playOrder.getOrNull(index + 1)
        val prevFits = prev != null && prev.albumId == album && order(prev) < order(e)
        val nextFits = next != null && next.albumId == album && order(next) > order(e)
        return prevFits || nextFits
    }

    private fun order(e: QueueEntry): Long = (e.discNumber ?: 1).toLong() * 10_000 + (e.track ?: 0)
}
