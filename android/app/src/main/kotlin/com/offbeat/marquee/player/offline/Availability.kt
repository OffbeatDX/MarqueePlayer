package com.offbeat.marquee.player.offline

import android.content.Context
import com.offbeat.marquee.player.diag.AppLog
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import com.offbeat.marquee.core.offline.Playable
import com.offbeat.marquee.core.playback.CacheKeys
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.playback.MediaStores
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * What can play without a network ([Playable]): downloaded songs, and songs whole in the cache.
 * The cache's list is read when the network goes (it can't grow while offline), so the UI and
 * [com.offbeat.marquee.player.playback.PlayerConnection] check it without touching the disk.
 */
object Availability {
    private const val TAG = "MarqueeAvailability"
    private lateinit var app: Context
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val _cached = MutableStateFlow<Set<String>>(emptySet())
    val cached: StateFlow<Set<String>> = _cached.asStateFlow()

    fun init(context: Context) {
        app = context.applicationContext
        scope.launch {
            NetworkMonitor.state.collect { s -> if (!s.online) refreshCached() }
        }
    }

    fun refreshCached() {
        val c = MediaStores.cache(app)
        val ids = c.keys.mapNotNullTo(HashSet()) { key -> CacheKeys.songId(key)?.takeIf { MediaStores.isComplete(c, key) } }
        _cached.value = ids
        AppLog.i(TAG, "offline: ${ids.size} songs whole in the cache, ${Downloads.stored.value.complete.size} downloaded")
    }

    fun canPlayOffline(songId: String): Boolean = Downloads.isDownloaded(songId) || songId in _cached.value

    fun canPlay(songId: String): Boolean = Playable.isPlayable(NetworkMonitor.online, Downloads.isDownloaded(songId), songId in _cached.value)
}

/** What a list draws for each song: dimmed when it can't play offline, a mark when downloaded. */
@Immutable
data class OfflineView(val online: Boolean = true, val downloaded: Set<String> = emptySet(), val cached: Set<String> = emptySet()) {
    fun isDownloaded(songId: String) = songId in downloaded
    fun canPlay(songId: String) = Playable.isPlayable(online, songId in downloaded, songId in cached)
}

val LocalOffline = staticCompositionLocalOf { OfflineView() }
