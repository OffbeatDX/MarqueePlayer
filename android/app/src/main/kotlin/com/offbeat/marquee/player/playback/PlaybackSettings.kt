package com.offbeat.marquee.player.playback

import android.content.Context
import android.content.SharedPreferences
import com.offbeat.marquee.core.playback.QualitySettings
import com.offbeat.marquee.core.playback.ReplayGainMode
import com.offbeat.marquee.core.playback.StreamQuality
import com.offbeat.marquee.core.settings.CacheCap
import com.offbeat.marquee.core.settings.PrefetchAhead
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The playback settings (Settings > Streaming, Cache, Playback), one per process, in
 * SharedPreferences `playback`. Each is a StateFlow: the playback service follows every change at
 * once (the next stream decision, the volume, the prefetch window, the cache cap, the scrobbler).
 */
object PlaybackSettings {
    private lateinit var prefs: SharedPreferences

    private val _quality = MutableStateFlow(QualitySettings())
    val qualityFlow: StateFlow<QualitySettings> = _quality.asStateFlow()
    private val _replayGain = MutableStateFlow(ReplayGainMode.DEFAULT)
    val replayGainFlow: StateFlow<ReplayGainMode> = _replayGain.asStateFlow()
    private val _prefetch = MutableStateFlow(PrefetchAhead.DEFAULT)
    val prefetchFlow: StateFlow<Int> = _prefetch.asStateFlow()
    private val _cacheCap = MutableStateFlow(CacheCap.bytes(CacheCap.DEFAULT_GB))
    val cacheCapFlow: StateFlow<Long> = _cacheCap.asStateFlow()
    private val _scrobbling = MutableStateFlow(true)
    val scrobblingFlow: StateFlow<Boolean> = _scrobbling.asStateFlow()

    /** Idempotent: the app calls it first; the cache may be opened before that from another entry point. */
    @Synchronized
    fun init(context: Context) {
        if (::prefs.isInitialized) return
        prefs = context.applicationContext.getSharedPreferences("playback", Context.MODE_PRIVATE)
        _quality.value = QualitySettings(
            unmetered = StreamQuality.fromId(prefs.getString(KEY_WIFI, null), StreamQuality.DEFAULT_WIFI),
            metered = StreamQuality.fromId(prefs.getString(KEY_MOBILE, null), StreamQuality.DEFAULT_MOBILE),
        )
        _replayGain.value = ReplayGainMode.fromName(prefs.getString(KEY_RG, null))
        _prefetch.value = PrefetchAhead.clamp(prefs.getInt(KEY_PREFETCH, PrefetchAhead.DEFAULT))
        _cacheCap.value = CacheCap.bytes(CacheCap.gbOf(prefs.getLong(KEY_CACHE_CAP, CacheCap.bytes(CacheCap.DEFAULT_GB))))
        _scrobbling.value = prefs.getBoolean(KEY_SCROBBLE, true)
    }

    var quality: QualitySettings
        get() = _quality.value
        set(v) {
            _quality.value = v
            prefs.edit().putString(KEY_WIFI, v.unmetered.id).putString(KEY_MOBILE, v.metered.id).apply()
        }

    var replayGain: ReplayGainMode
        get() = _replayGain.value
        set(v) {
            _replayGain.value = v
            prefs.edit().putString(KEY_RG, v.name).apply()
        }

    /** How many tracks after the current one are fetched whole ahead of time (0 to 5). */
    var prefetchCount: Int
        get() = _prefetch.value
        set(v) {
            val n = PrefetchAhead.clamp(v)
            _prefetch.value = n
            prefs.edit().putInt(KEY_PREFETCH, n).apply()
        }

    /** The cache's cap; lowering it trims the cache at once ([MediaStores]). */
    var cacheCapBytes: Long
        get() = _cacheCap.value
        set(v) {
            _cacheCap.value = v
            prefs.edit().putLong(KEY_CACHE_CAP, v).apply()
        }

    var scrobbling: Boolean
        get() = _scrobbling.value
        set(v) {
            _scrobbling.value = v
            prefs.edit().putBoolean(KEY_SCROBBLE, v).apply()
        }

    /**
     * Audio offload (hardware decoding while the CPU sleeps). Not a setting on screen: the EQ turns
     * it off while it is on, since offloaded audio can't go through an AudioProcessor.
     */
    val audioOffload: Boolean
        get() = prefs.getBoolean(KEY_OFFLOAD, true)

    private const val KEY_WIFI = "quality_wifi"
    private const val KEY_MOBILE = "quality_mobile"
    private const val KEY_RG = "replay_gain"
    private const val KEY_PREFETCH = "prefetch_count"
    private const val KEY_CACHE_CAP = "cache_cap_bytes"
    private const val KEY_SCROBBLE = "scrobbling"
    private const val KEY_OFFLOAD = "audio_offload"
}
