package com.offbeat.marquee.player.widget

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import com.offbeat.marquee.player.diag.AppLog
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.glance.appwidget.updateAll
import androidx.media3.common.Player
import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.core.art.CoverUris
import com.offbeat.marquee.player.art.Covers
import com.offbeat.marquee.player.playback.SongItems
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/** What the widget shows: the current song and whether it plays. Saved, so it survives the process. */
@Serializable
data class WidgetState(
    val hasItem: Boolean = false,
    val songId: String = "",
    val title: String = "",
    val artist: String = "",
    val coverId: String? = null,
    val playing: Boolean = false,
) {
    companion object {
        /** The state a player is in: playing means play is wanted (buffering included), so it doesn't flicker. */
        fun of(p: Player): WidgetState {
            val item = p.currentMediaItem ?: return WidgetState(playing = false)
            val m = item.mediaMetadata
            return WidgetState(
                hasItem = true,
                songId = item.mediaId,
                title = m.title?.toString().orEmpty(),
                artist = m.artist?.toString().orEmpty(),
                coverId = SongItems.song(item)?.coverArt ?: Covers.coverIdOf(m.artworkUri),
                playing = p.playWhenReady && p.playbackState != Player.STATE_ENDED,
            )
        }
    }
}

object WidgetStore {
    private const val PREFS = "widget"
    private const val KEY = "state"
    private val json = Json { ignoreUnknownKeys = true }

    fun load(context: Context): WidgetState =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, null)
            ?.let { runCatching { json.decodeFromString(WidgetState.serializer(), it) }.getOrNull() } ?: WidgetState()

    fun save(context: Context, s: WidgetState) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(KEY, json.encodeToString(WidgetState.serializer(), s)).apply()
    }

    /**
     * The cover as a small bitmap (the 300 bucket, from the cover cache; fetched once when missing).
     * Blocking: call it off the main thread.
     */
    fun cover(context: Context, coverId: String?): Bitmap? {
        coverId ?: return null
        val file = Covers.file(context, CoverUris.Request(coverId, CoverSize.GRID)) ?: return null
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(file.path, bounds)
        var sample = 1
        while (bounds.outWidth / (sample * 2) >= CoverSize.GRID && bounds.outHeight / (sample * 2) >= CoverSize.GRID) sample *= 2
        return BitmapFactory.decodeFile(file.path, BitmapFactory.Options().apply { inSampleSize = sample })
    }
}

/**
 * The playback service's side: a new widget state is pushed only when what the widget shows changes
 * (the track, play or pause; the accent follows the cover), at most once per 300 ms burst of player
 * events. Nothing runs on a timer, and nothing at all while no widget is placed.
 */
class WidgetPusher(private val context: Context, private val scope: CoroutineScope) {
    private var last: WidgetState = WidgetStore.load(context)
    private var job: Job? = null
    private var pushes = 0

    fun onPlayer(p: Player) {
        val s = WidgetState.of(p)
        if (s == last) return
        last = s
        job?.cancel()
        job = scope.launch {
            delay(DEBOUNCE_MS)
            withContext(Dispatchers.IO) { WidgetStore.save(context, s) }
            val placed = runCatching { GlanceAppWidgetManager(context).getGlanceIds(PlayerWidget::class.java) }.getOrDefault(emptyList())
            if (placed.isEmpty()) {
                // A widget placed later starts from the saved state.
                WidgetLive.data.value = null
                return@launch
            }
            // Cover and accent first, so a running Glance session recomposes once, complete.
            WidgetLive.data.value = WidgetLive.load(context, s)
            pushes++
            AppLog.i(TAG, "widget push #$pushes: ${if (!s.hasItem) "empty" else if (s.playing) "playing ${s.songId}" else "paused ${s.songId}"} (${placed.size} placed)")
            PlayerWidget().updateAll(context)
        }
    }

    /** The service is going away: whatever was playing has stopped, so the widget shows Play. */
    fun onServiceStopped() {
        job?.cancel()
        val s = last.copy(playing = false)
        if (s == last) return
        last = s
        WidgetStore.save(context, s)
        WidgetLive.data.value = WidgetLive.data.value?.let { WidgetData(s, it.cover, it.accent, it.pureBlack) }
        // The service's scope ends with it; the update outlives it (the process usually does too).
        CoroutineScope(Dispatchers.Main + SupervisorJob()).launch {
            if (runCatching { GlanceAppWidgetManager(context).getGlanceIds(PlayerWidget::class.java) }.getOrDefault(emptyList()).isNotEmpty()) {
                AppLog.i(TAG, "widget push: service stopped, paused")
                PlayerWidget().updateAll(context)
            }
        }
    }

    private companion object {
        const val TAG = "MarqueeWidget"
        const val DEBOUNCE_MS = 300L
    }
}
