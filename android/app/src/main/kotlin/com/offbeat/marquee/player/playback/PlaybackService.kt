package com.offbeat.marquee.player.playback

import android.app.PendingIntent
import android.content.Intent
import android.media.audiofx.AudioEffect
import android.os.Bundle
import android.os.SystemClock
import android.util.LruCache
import androidx.annotation.OptIn
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.Timeline
import androidx.media3.common.TrackSelectionParameters.AudioOffloadPreferences
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSourceBitmapLoader
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.HttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.exoplayer.source.ShuffleOrder
import androidx.media3.session.CacheBitmapLoader
import androidx.media3.session.CommandButton
import androidx.media3.session.LibraryResult
import androidx.media3.session.MediaLibraryService
import androidx.media3.session.MediaSession
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionError
import androidx.media3.session.SessionResult
import com.google.common.collect.ImmutableList
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import com.google.common.util.concurrent.SettableFuture
import com.offbeat.marquee.core.auto.BrowseId
import com.offbeat.marquee.core.auto.ControllerTrust
import com.offbeat.marquee.core.auto.BrowsePaging
import com.offbeat.marquee.core.eq.EqSetting
import com.offbeat.marquee.core.offline.Playable
import com.offbeat.marquee.core.playback.PlaybackRecovery
import com.offbeat.marquee.core.playback.QueueOrder
import com.offbeat.marquee.core.playback.ReplayGainMath
import com.offbeat.marquee.core.playback.ReplayGainMode
import com.offbeat.marquee.core.playback.SavedQueue
import com.offbeat.marquee.core.playback.Scrobbler
import com.offbeat.marquee.core.playback.SeekReload
import com.offbeat.marquee.core.playback.SleepMode
import com.offbeat.marquee.core.playback.SleepTimer
import com.offbeat.marquee.core.playback.StallRule
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.BuildConfig
import com.offbeat.marquee.player.MainActivity
import com.offbeat.marquee.player.R
import com.offbeat.marquee.player.art.Covers
import com.offbeat.marquee.player.data.Item
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.diag.AppLog
import com.offbeat.marquee.player.eq.EqStore
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.session.AppSession
import com.offbeat.marquee.player.sync.Sync
import com.offbeat.marquee.player.widget.WidgetPusher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * Owns the one ExoPlayer, its MediaLibrarySession, the cache and prefetcher, ReplayGain, the
 * scrobbler, the sleep timer and the saved queue. The UI only ever talks to it through a
 * MediaController. Media3 runs it as a foreground service (mediaPlayback) while playing and posts
 * the stock media notification.
 */
@OptIn(UnstableApi::class)
class PlaybackService : MediaLibraryService() {
    private val settings = PlaybackSettings
    private lateinit var source: SongSource
    private lateinit var player: ExoPlayer
    /** What the session hands to controllers: [player], with seeks in an unseekable track routed to [onSeekInItem]. */
    private lateinit var sessionPlayer: SeekingPlayer
    private lateinit var session: MediaLibrarySession
    private lateinit var prefetcher: Prefetcher
    private lateinit var queueStore: QueueStore
    private lateinit var scrobbler: Scrobbler
    private lateinit var sleepTimer: SleepTimer
    /** The notification's cover loader; shut down with the service, or its thread outlives it. */
    private lateinit var bitmapExecutor: ExecutorService

    private val scope = MainScope()
    @kotlin.OptIn(ExperimentalCoroutinesApi::class)
    private val io = Dispatchers.IO.limitedParallelism(1)
    private val songs = LruCache<String, Song>(4096)
    /** The browse tree and search for Android Auto. */
    private val auto = AutoLibrary()
    private lateinit var widget: WidgetPusher

    private var replayGainVolume = 1f
    /** The built-in equalizer, in the audio sink's processor chain; active only while it is on. */
    private val eqProcessor = EqAudioProcessor()
    private var eqSink: EqAudioSink? = null
    private var eqOn: Boolean? = null
    /** The audio session handed to system effects (Wavelet and the like). */
    private var effectSession = C.AUDIO_SESSION_ID_UNSET
    private var fadeVolume = 1f
    private var periodicSave: Job? = null
    private var saveJob: Job? = null
    private var restored = false
    /** A saved play order waiting for its items (playback resumption hands the items over first). */
    private var pendingShuffleOrder: List<Int>? = null
    /** What controllers read from the session: the sleep timer and the current song's favourite state. */
    private val extras = Bundle()

    override fun onCreate() {
        super.onCreate()
        PlaybackSettings.init(this)
        // The system's media controls read the session's artwork URIs without connecting through
        // onConnect on Android 12 (and some makers' builds), so they never got the grant: logcat filled
        // with "Permission Denial: opening provider ...CoverProvider" from com.android.systemui.
        Covers.grantTo(this, "com.android.systemui")
        widget = WidgetPusher(this, scope)
        source = SongSource(this, settings)
        queueStore = QueueStore(this)

        eqProcessor.onReconfigureNeeded = { eqSink?.requestReconfigure() }
        player = ExoPlayer.Builder(this, EqAudioSink.Renderers(this, eqProcessor) { eqSink = it })
            .setMediaSourceFactory(DefaultMediaSourceFactory(source.playerDataSourceFactory))
            .setAudioAttributes(
                AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MUSIC).build(),
                /* handleAudioFocus = */ true,
            )
            .setHandleAudioBecomingNoisy(true)
            .setWakeMode(C.WAKE_MODE_NETWORK)
            .build()
        // The equalizer's setting before the first track configures the sink; offload follows it.
        applyEq(EqStore.setting.value)
        player.addListener(playerListener)
        openEffectSession(player.audioSessionId)
        if (BuildConfig.DEBUG) player.addAnalyticsListener(DebugLoadLogger())
        player.addAudioOffloadListener(object : ExoPlayer.AudioOffloadListener {
            override fun onOffloadedPlayback(offloaded: Boolean) {
                AppLog.i(TAG, "audio offload ${if (offloaded) "on" else "off"}")
            }
        })

        // Artwork URIs are the cover provider's content URIs: read through the provider like any app
        // would, so the notification shares the cover cache with the screens.
        bitmapExecutor = Executors.newSingleThreadExecutor()
        val bitmapLoader = CacheBitmapLoader(
            DataSourceBitmapLoader(
                MoreExecutors.listeningDecorator(bitmapExecutor),
                DefaultDataSource.Factory(this),
            ),
        )
        sessionPlayer = SeekingPlayer(player, ::onSeekInItem)
        session = MediaLibrarySession.Builder(this, sessionPlayer, sessionCallback)
            .setSessionActivity(
                PendingIntent.getActivity(
                    this, 0, MainActivity.nowPlayingIntent(this),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                ),
            )
            .setBitmapLoader(bitmapLoader)
            .build()

        prefetcher = Prefetcher(
            source, scope,
            onFetching = { fetching ->
                val mode = if (fetching) C.WAKE_MODE_NETWORK else C.WAKE_MODE_LOCAL
                player.setWakeMode(mode)
                AppLog.i(TAG, "wake mode ${if (fetching) "network" else "local"}")
            },
            // The radio is up for a track: whatever waits in the outbox goes with it.
            onNetworkUsed = { Sync.flushIfDue("prefetch") },
            onCached = { id -> if (id == player.currentMediaItem?.mediaId) recheckSeek("track fetched") },
        )
        scrobbler = Scrobbler(
            scope = scope,
            clock = SystemClock::elapsedRealtime,
            send = { id, time, submission ->
                // Only "now playing" comes here (submissions are queued): offline it is dropped.
                if (!NetworkMonitor.online) throw SubsonicError.Network(SubsonicError.Network.Kind.Other, null)
                val c = AppSession.current ?: throw SubsonicError.Network(SubsonicError.Network.Kind.Other, null)
                c.scrobble(id, time, submission)
            },
            enabled = { settings.scrobbling },
            log = { AppLog.i(TAG, it) },
            queue = { id, time -> Sync.queueScrobble(id, time) },
        )
        sleepTimer = SleepTimer(
            scope = scope,
            clock = SystemClock::elapsedRealtime,
            onFade = { fadeVolume = it; applyVolume() },
            onExpire = {
                AppLog.i(TAG, "sleep timer: time is up, pausing")
                player.pause()
            },
        )
        scope.launch { sleepTimer.mode.collect(::onSleepModeChanged) }
        updateButtons()

        scope.launch {
            val saved = withContext(Dispatchers.IO) {
                AppSession.ensureLoaded(this@PlaybackService)
                MediaStores.healCacheOnce(this@PlaybackService)
                queueStore.load()
            }
            restoreIfEmpty(saved)
            restored = true
        }
        scope.launch {
            var signedIn = false
            AppSession.client.collect { c ->
                if (c == null && signedIn) {
                    AppLog.i(TAG, "signed out: clearing the queue")
                    stopPeriodicSave()
                    saveJob?.cancel()
                    recovery.reset()
                    player.stop()
                    player.clearMediaItems()
                    prefetcher.stop()
                    auto.clear()
                    val seq = queueStore.nextSeq()
                    withContext(io) { queueStore.clear(seq) }
                }
                signedIn = c != null
            }
        }
        // A different network: the streams still open were opened on the old one and may never
        // answer again. They are cancelled so the player and the prefetcher retry on the new one.
        StreamHttp.shortReadTimeout = stallFixes
        AppLog.i(TAG, "stall fixes ${if (stallFixes) "on" else "off (debug comparison)"}")
        scope.launch {
            NetworkMonitor.switched.collect {
                if (stallFixes) {
                    StreamHttp.dropConnections("network changed")
                    source.redecideUnfetched(playWindow(1).mapTo(HashSet()) { player.getMediaItemAt(it).mediaId })
                    // After the old connections are gone (dropped off the main thread).
                    scope.launch { delay(500); prefetcher.retry() }
                } else {
                    AppLog.i(TAG, "network changed (stall fixes off: connections kept)")
                }
            }
        }
        scope.launch { NetworkMonitor.cameBack.collect { onNetworkBack() } }
        // The heart follows changes made anywhere (the UI, the outbox, a restart).
        scope.launch { Library.favourites.collect { updateButtons() } }
        // A download that just finished becomes the copy to play, when the player opens the song
        // afresh anyway: idle, or about to load it again for a seek that waits. While it reads the
        // song, its next open (a seek, a retry) would read the downloaded copy, perhaps another
        // quality, at the other copy's byte offset; it plays the next time the song is decided.
        scope.launch {
            Downloads.stored.collect { s ->
                currentSong()?.id?.let {
                    val reopens = player.playbackState == Player.STATE_IDLE ||
                        (seekReload.pendingMs != null && seekItem()?.let { i -> i.prepared && !i.placeholder && !i.seekable } == true)
                    if (it in s.complete && reopens) {
                        source.forget(it)
                        recheckSeek("track downloaded")
                    }
                }
            }
        }
        // The equalizer, live while its sliders move (the audio thread takes the latest at its next buffer).
        scope.launch { EqStore.setting.collect(::applyEq) }
        // Settings, followed as they change (Settings > Streaming, Cache, Playback).
        scope.launch {
            settings.replayGainFlow.drop(1).collect {
                AppLog.i(TAG, "setting: replaygain ${it.name.lowercase()}")
                updateReplayGain()
            }
        }
        scope.launch {
            settings.prefetchFlow.drop(1).collect {
                AppLog.i(TAG, "setting: prefetch $it ahead")
                updatePrefetch()
            }
        }
        scope.launch {
            settings.qualityFlow.drop(1).collect { q ->
                AppLog.i(TAG, "setting: quality wi-fi ${q.unmetered.id}, mobile ${q.metered.id}")
                // The songs not started yet (nothing stored) are decided again with the new quality.
                source.redecideUnfetched(playWindow(1).mapTo(HashSet()) { player.getMediaItemAt(it).mediaId }, "quality changed")
                if (player.playWhenReady) prefetcher.restart("quality changed")
            }
        }
        scope.launch { settings.scrobblingFlow.drop(1).collect { AppLog.i(TAG, "setting: scrobbling ${if (it) "on" else "off"}") } }
    }

    /** The equalizer's setting to the processor; offload off while it is on, back on (if allowed) when off. */
    private fun applyEq(setting: EqSetting) {
        eqProcessor.update(setting.params())
        if (setting.enabled != eqOn) {
            eqOn = setting.enabled
            val offload = settings.audioOffload && !setting.enabled
            setAudioOffload(offload)
            AppLog.i(TAG, "equalizer ${if (setting.enabled) "on (${setting.label})" else "off"}: audio offload preference ${if (offload) "enabled" else "disabled"}")
        }
    }

    /** Hands the player's audio session to system effects (an equalizer app can attach to it): one broadcast, nothing running. */
    private fun openEffectSession(id: Int) {
        if (id == effectSession) return
        closeEffectSession()
        if (id == C.AUDIO_SESSION_ID_UNSET) return
        sendBroadcast(
            Intent(AudioEffect.ACTION_OPEN_AUDIO_EFFECT_CONTROL_SESSION)
                .putExtra(AudioEffect.EXTRA_AUDIO_SESSION, id)
                .putExtra(AudioEffect.EXTRA_PACKAGE_NAME, packageName)
                .putExtra(AudioEffect.EXTRA_CONTENT_TYPE, AudioEffect.CONTENT_TYPE_MUSIC),
        )
        effectSession = id
        AppLog.i(TAG, "audio session $id open for system effects")
    }

    private fun closeEffectSession() {
        if (effectSession == C.AUDIO_SESSION_ID_UNSET) return
        sendBroadcast(
            Intent(AudioEffect.ACTION_CLOSE_AUDIO_EFFECT_CONTROL_SESSION)
                .putExtra(AudioEffect.EXTRA_AUDIO_SESSION, effectSession)
                .putExtra(AudioEffect.EXTRA_PACKAGE_NAME, packageName),
        )
        AppLog.i(TAG, "audio session $effectSession closed for system effects")
        effectSession = C.AUDIO_SESSION_ID_UNSET
    }

    private fun onNetworkBack() {
        prefetcher.retry()
        if (waitingForNetwork) {
            waitingForNetwork = false
            AppLog.i(TAG, "recovery: network back, preparing again")
            if (player.playbackState == Player.STATE_IDLE && player.mediaItemCount > 0) player.prepare()
        }
    }

    // ---- Recovery: retries, skipping what can't play, the buffering watchdog -------------------

    private val recovery = PlaybackRecovery()

    /**
     * Debug builds only: a file `debug-no-stall-fix` in the app's files turns off the two stall fixes
     * (dropping connections when the network changes, the watchdog's re-prepare), so the stall can be
     * reproduced and compared. The watchdog still logs what it sees.
     */
    private val stallFixes by lazy { !(BuildConfig.DEBUG && java.io.File(filesDir, "debug-no-stall-fix").exists()) }
    private var waitingForNetwork = false
    private var retryJob: Job? = null
    private var stallJob: Job? = null
    private var stallRecoveries = 0
    private var stallTrack: Long = -1

    private fun itemKey(item: MediaItem) = "${item.mediaId}#${SongItems.uid(item)}"

    /** Walks the cause chain for the HTTP status code when Media3 reported a bad HTTP response. */
    private fun httpStatusOf(e: Throwable): Int? {
        var cur: Throwable? = e
        val seen = HashSet<Throwable>()
        while (cur != null && seen.add(cur)) {
            if (cur is HttpDataSource.InvalidResponseCodeException) return cur.responseCode
            cur = cur.cause
        }
        return null
    }

    private fun classifyError(e: PlaybackException): PlaybackRecovery.FailureKind {
        val reply = ErrorReplyException.findIn(e)
        return PlaybackRecovery.classify(httpStatusOf(e), e.errorCode in NETWORK_IO_ERRORS, errorReply = reply != null, subsonicCode = reply?.code)
    }

    private fun pauseNotice(kind: PlaybackRecovery.FailureKind): String = getString(
        when (kind) {
            PlaybackRecovery.FailureKind.Auth -> R.string.notice_auth_failed
            PlaybackRecovery.FailureKind.Retryable -> R.string.notice_paused_server_error
            PlaybackRecovery.FailureKind.NotFound -> R.string.notice_paused_tracks_missing
            PlaybackRecovery.FailureKind.Other -> R.string.notice_paused_tracks_unplayable
        },
    )

    private fun onError(error: PlaybackException) {
        val item = player.currentMediaItem ?: return
        retryJob?.cancel()
        val online = NetworkMonitor.online
        if (!online && !source.isStoredAnyQuality(item.mediaId)) {
            // Offline mid-track: on to something stored if there is one, else wait and carry on.
            if (!skipToPlayable("not stored and offline", pauseIfNone = false)) {
                AppLog.i(TAG, "recovery: ${item.mediaId} can't load offline and nothing else is stored: waiting for the network")
                waitingForNetwork = true
            }
            return
        }
        val kind = classifyError(error)
        when (val action = recovery.onError(itemKey(item), kind, online)) {
            is PlaybackRecovery.Action.Retry -> {
                AppLog.i(TAG, "recovery: failure ${action.failures} on ${item.mediaId}, retrying in ${action.afterMs} ms")
                retryJob = scope.launch {
                    delay(action.afterMs)
                    if (player.playbackState == Player.STATE_IDLE && player.currentMediaItem?.let(::itemKey) == itemKey(item)) player.prepare()
                }
            }
            is PlaybackRecovery.Action.WaitForNetwork -> {
                AppLog.i(TAG, "recovery: failure ${action.failures} on ${item.mediaId}, waiting for the network")
                waitingForNetwork = true
            }
            PlaybackRecovery.Action.Skip -> {
                notice("Couldn't play “${item.mediaMetadata.title}”, skipped")
                skipToPlayable("failed ${if (kind == PlaybackRecovery.FailureKind.Retryable) "3 times" else "(${error.errorCodeName})"}")
            }
            is PlaybackRecovery.Action.Pause -> {
                AppLog.i(TAG, "recovery: pausing (${action.kind.name.lowercase()}) on ${item.mediaId}")
                waitingForNetwork = false
                player.pause()
                notice(pauseNotice(action.kind))
            }
        }
    }

    private fun canPlayNow(index: Int): Boolean =
        NetworkMonitor.online || source.isStoredAnyQuality(player.getMediaItemAt(index).mediaId)

    /** Moves on to the next entry that can play (offline: a stored one); pauses when there is none. */
    private fun skipToPlayable(why: String, pauseIfNone: Boolean = true): Boolean {
        val current = player.currentMediaItemIndex
        val order = if (player.shuffleModeEnabled) shuffleOrder() else QueueOrder.identity(player.mediaItemCount)
        val next = Playable.nextIn(order, current, wrap = player.repeatMode == Player.REPEAT_MODE_ALL) { it != current && canPlayNow(it) }
        if (next == null) {
            if (!pauseIfNone) return false
            AppLog.i(TAG, "skip ($why): nothing else can play, pausing")
            player.pause()
            if (!NetworkMonitor.online) notice("Nothing else in the queue is downloaded")
            return false
        }
        AppLog.i(TAG, "skip ($why): ${player.getMediaItemAt(current).mediaId} -> ${player.getMediaItemAt(next).mediaId}")
        player.seekTo(next, 0)
        if (player.playbackState == Player.STATE_IDLE) player.prepare()
        return true
    }

    /** Offline, the current entry must be stored: otherwise on to the next one that is. */
    private fun ensurePlayableOffline() {
        if (NetworkMonitor.online || player.mediaItemCount == 0 || !player.playWhenReady) return
        val item = player.currentMediaItem ?: return
        if (source.isStoredAnyQuality(item.mediaId)) return
        skipToPlayable("offline, not stored")
    }

    /**
     * Arms the watchdog while buffering with play wanted, disarms it otherwise. Called on state and
     * play-when-ready changes: a new queue goes BUFFERING before play() sets play-when-ready.
     */
    private fun updateWatchdog() {
        val buffering = player.playbackState == Player.STATE_BUFFERING && player.playWhenReady
        if (!buffering) {
            stallJob?.cancel()
            stallJob = null
            return
        }
        if (stallJob?.isActive == true) return
        val since = SystemClock.elapsedRealtime()
        stallJob = scope.launch {
            delay(StallRule.AFTER_MS)
            checkStall(since)
        }
    }

    private fun onPlaybackState(state: Int) {
        AppLog.i(TAG, "state: ${stateName(state)}" + if (state == Player.STATE_BUFFERING || state == Player.STATE_READY) " (${if (player.playWhenReady) "play" else "paused"})" else "")
        if (state == Player.STATE_READY && player.isPlaying) player.currentMediaItem?.let { recovery.onPlaying(itemKey(it)) }
        updateWatchdog()
    }

    private fun stateName(state: Int) = when (state) {
        Player.STATE_IDLE -> "idle"
        Player.STATE_BUFFERING -> "buffering"
        Player.STATE_READY -> "ready"
        Player.STATE_ENDED -> "ended"
        else -> "$state"
    }

    /** Buffering for a while: with the whole track on the phone it is a stall (see [StallRule]). */
    private fun checkStall(since: Long) {
        if (player.playbackState != Player.STATE_BUFFERING) return
        val item = player.currentMediaItem ?: return
        val uid = SongItems.uid(item)
        if (uid != stallTrack) {
            stallTrack = uid
            stallRecoveries = 0
        }
        val key = source.keyFor(item.mediaId)
        val stored = source.isStored(key)
        val forMs = SystemClock.elapsedRealtime() - since
        if (!StallRule.shouldRecover(forMs, player.playWhenReady, stored, stallRecoveries)) {
            AppLog.i(TAG, "watchdog: buffering $forMs ms on ${item.mediaId}, ${if (stored) "stored" else "${source.cachedBytes(key)} bytes cached so far"}; waiting")
            if (!stored) stallJob = scope.launch { delay(StallRule.AFTER_MS); checkStall(since) }
            return
        }
        if (!stallFixes) {
            AppLog.w(TAG, "watchdog: buffering $forMs ms with ${item.mediaId} fully stored (stall fixes off: not acting)")
            stallJob = scope.launch { delay(StallRule.AFTER_MS); checkStall(since) }
            return
        }
        stallRecoveries++
        val position = player.currentPosition
        AppLog.w(TAG, "watchdog: buffering $forMs ms with ${item.mediaId} fully stored: dropping its connection, re-preparing at $position ms")
        StreamHttp.dropConnections("stall")
        val index = player.currentMediaItemIndex
        player.stop()
        player.seekTo(index, position)
        player.prepare()
    }

    // ---- Seeking in a track that can't seek (a live transcode), see SeekReload ----------------

    private val seekReload = SeekReload()
    private val timelineWindow = Timeline.Window()

    /** The current entry for [SeekReload]: whether it seeks, and whether it is whole on the phone. */
    private fun seekItem(): SeekReload.Item? {
        val item = player.currentMediaItem ?: return null
        val tl = player.currentTimeline
        if (tl.isEmpty) return null
        val w = tl.getWindow(player.currentMediaItemIndex, timelineWindow)
        val prepared = player.playbackState != Player.STATE_IDLE
        // Asked only when it matters: it decides the song's quality if that isn't decided yet.
        val stored = prepared && !w.isPlaceholder && !w.isSeekable && source.isStored(source.keyFor(item.mediaId))
        return SeekReload.Item(itemKey(item), prepared, w.isPlaceholder, w.isSeekable, stored)
    }

    /** From [SeekingPlayer], every seek inside the current entry: true when it is dealt with here. */
    private fun onSeekInItem(positionMs: Long): Boolean {
        val item = seekItem() ?: return false
        val action = seekReload.seek(item, positionMs)
        if (action.kind == SeekReload.Kind.Seek) {
            if (item.prepared && !item.seekable) AppLog.i(TAG, "seek: ${player.currentMediaItem?.mediaId} back to the start (the stream starts again)")
            updatePendingSeekExtra()
            return false
        }
        act(action, "seek")
        return true
    }

    /** The player knows more (the timeline, or it is ready): a seek that waited may go now. */
    private fun onSourceKnown() {
        if (player.playbackState == Player.STATE_READY) {
            val item = seekItem() ?: return
            if (seekReload.onReady(item)) {
                AppLog.w(TAG, "seek: ${player.currentMediaItem?.mediaId} loaded again from the stored copy and still can't seek")
            }
        }
        recheckSeek("source known")
    }

    private fun recheckSeek(why: String) {
        if (seekReload.pendingMs == null) return
        val item = seekItem() ?: return
        seekReload.recheck(item)?.let { act(it, why) }
        updatePendingSeekExtra()
    }

    private fun act(action: SeekReload.Action, why: String) {
        val id = player.currentMediaItem?.mediaId
        val at = action.positionMs
        when (action.kind) {
            SeekReload.Kind.Seek -> {
                AppLog.i(TAG, "seek: $id can seek now ($why), to $at ms")
                player.seekTo(at)
            }
            SeekReload.Kind.Reload -> {
                // Stop and prepare again: the item is loaded afresh, from the stored copy (its length
                // known, so it seeks), starting at the position. The queue, play-when-ready, the
                // shuffle order, repeat mode and volume are the player's and stay; the item doesn't
                // change, so it is no new play (no transition). An equal item given to
                // replaceMediaItem would only update its metadata, without loading it again.
                AppLog.i(TAG, "seek: $id can't seek (live stream), loading it again from the stored copy at $at ms ($why)")
                val index = player.currentMediaItemIndex
                player.stop()
                player.seekTo(index, at)
                player.prepare()
            }
            SeekReload.Kind.WaitForTrack -> {
                AppLog.i(TAG, "seek: $id can't seek (live stream) and isn't whole on the phone yet: seeking to $at ms once it is")
                notice("Seeking to ${clock(at)} once the track has loaded")
                // The current track comes first in the prefetch window; fetched even while paused.
                updatePrefetch(evenPaused = true)
            }
            SeekReload.Kind.WaitForSource -> AppLog.i(TAG, "seek: $id is opening, seeking to $at ms once it is known whether it seeks")
            SeekReload.Kind.Ignore -> {
                AppLog.w(TAG, "seek: $id can't seek, even from the stored copy: ignored")
                notice("Can't seek in this track")
            }
        }
        updatePendingSeekExtra()
    }

    /** The position a seek waits to go to, for the UI's seek bar (absent when none waits). */
    private fun updatePendingSeekExtra() {
        val ms = seekReload.pendingMs
        val had = extras.containsKey(PlaybackCommands.EXTRA_PENDING_SEEK_MS)
        if (ms == null && !had) return
        if (ms != null && had && extras.getLong(PlaybackCommands.EXTRA_PENDING_SEEK_MS) == ms) return
        if (ms == null) extras.remove(PlaybackCommands.EXTRA_PENDING_SEEK_MS) else extras.putLong(PlaybackCommands.EXTRA_PENDING_SEEK_MS, ms)
        session.setSessionExtras(Bundle(extras))
    }

    private fun clock(ms: Long): String = "%d:%02d".format(ms / 60_000, ms / 1000 % 60)

    /** A short line for the UI's snackbar (controllers get it as a custom command). */
    private fun notice(text: String) {
        AppLog.i(TAG, "notice: $text")
        session.broadcastCustomCommand(PlaybackCommands.notice, Bundle().apply { putString(PlaybackCommands.ARG_TEXT, text) })
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaLibrarySession = session

    override fun onDestroy() {
        stopPeriodicSave()
        saveJob?.cancel()
        if (restored) saveNow()
        widget.onServiceStopped()
        scrobbler.onStopped()
        prefetcher.stop()
        sleepTimer.cancel()
        session.release()
        bitmapExecutor.shutdown()
        closeEffectSession()
        player.release()
        // Nothing is read any more: removals held back for the songs it was on happen now.
        Downloads.setPlaying(emptySet())
        scope.cancel()
        super.onDestroy()
    }

    /** Offload on or off: off while the equalizer is on (processors don't run on offloaded audio). */
    private fun setAudioOffload(enabled: Boolean) {
        val prefs = if (enabled) {
            AudioOffloadPreferences.Builder()
                .setAudioOffloadMode(AudioOffloadPreferences.AUDIO_OFFLOAD_MODE_ENABLED)
                .setIsGaplessSupportRequired(true)
                .build()
        } else {
            AudioOffloadPreferences.DEFAULT
        }
        player.trackSelectionParameters = player.trackSelectionParameters.buildUpon().setAudioOffloadPreferences(prefs).build()
    }

    // ---- Player events ------------------------------------------------------------------

    private val playerListener = object : Player.Listener {
        override fun onAudioSessionIdChanged(audioSessionId: Int) = openEffectSession(audioSessionId)

        override fun onEvents(p: Player, events: Player.Events) {
            if (events.containsAny(
                    Player.EVENT_MEDIA_ITEM_TRANSITION, Player.EVENT_TIMELINE_CHANGED,
                    Player.EVENT_SHUFFLE_MODE_ENABLED_CHANGED, Player.EVENT_REPEAT_MODE_CHANGED,
                    Player.EVENT_PLAY_WHEN_READY_CHANGED,
                )
            ) {
                updatePrefetch()
                updateReplayGain()
            }
            if (events.contains(Player.EVENT_MEDIA_ITEM_TRANSITION)) updateButtons()
            if (events.containsAny(
                    Player.EVENT_MEDIA_ITEM_TRANSITION, Player.EVENT_PLAY_WHEN_READY_CHANGED,
                    Player.EVENT_PLAYBACK_STATE_CHANGED, Player.EVENT_TIMELINE_CHANGED,
                )
            ) {
                widget.onPlayer(p)
            }
            if (events.contains(Player.EVENT_PLAY_WHEN_READY_CHANGED) && p.playWhenReady) ensurePlayableOffline()
            if (events.contains(Player.EVENT_PLAY_WHEN_READY_CHANGED)) updateWatchdog()
            if (events.containsAny(Player.EVENT_TIMELINE_CHANGED, Player.EVENT_PLAYBACK_STATE_CHANGED)) onSourceKnown()
            if (events.containsAny(Player.EVENT_MEDIA_ITEM_TRANSITION, Player.EVENT_IS_PLAYING_CHANGED, Player.EVENT_PLAY_WHEN_READY_CHANGED)) {
                updateScrobble()
            }
            if (events.contains(Player.EVENT_IS_PLAYING_CHANGED)) {
                if (p.isPlaying) startPeriodicSave() else stopPeriodicSave()
            }
            // Pause (or stop), track change, queue edit: save.
            if (restored && (
                    events.contains(Player.EVENT_MEDIA_ITEM_TRANSITION) || events.contains(Player.EVENT_TIMELINE_CHANGED) ||
                        (events.contains(Player.EVENT_IS_PLAYING_CHANGED) && !p.isPlaying)
                    )
            ) {
                save()
            }
        }

        override fun onTimelineChanged(timeline: Timeline, reason: Int) {
            val pending = pendingShuffleOrder ?: return
            if (pending.size == player.mediaItemCount) {
                pendingShuffleOrder = null
                setPlayOrder(pending)
                AppLog.i(TAG, "restored the saved shuffle order")
            }
        }

        override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
            if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT) scrobbler.onRepeat()
            if (seekReload.pendingMs != null) AppLog.i(TAG, "seek: another track now, the seek that waited is dropped")
            seekReload.onOtherItem()
            updatePendingSeekExtra()
            AppLog.i(TAG, "now: ${mediaItem?.mediaId} (${transitionReason(reason)})")
            ensurePlayableOffline()
        }

        override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) {
            // Paused through deep sleep, the timer's delay stood still: its end is checked on the real clock.
            if (playWhenReady && sleepTimer.recheck()) AppLog.i(TAG, "sleep timer: its time ran out while paused, cancelled")
            if (!playWhenReady && reason == Player.PLAY_WHEN_READY_CHANGE_REASON_END_OF_MEDIA_ITEM &&
                sleepTimer.mode.value == SleepMode.EndOfTrack
            ) {
                AppLog.i(TAG, "sleep timer: end of track, paused")
                sleepTimer.cancel()
            }
        }

        override fun onPlayerError(error: PlaybackException) {
            // The error's cause can carry a request; log only its kind or HTTP status.
            val detail = httpStatusOf(error)?.let { "HTTP $it" }
                ?: ErrorReplyException.findIn(error)?.let { "a Subsonic error answer, code ${it.code ?: "none"}" }
                ?: error.cause?.javaClass?.simpleName
            AppLog.w(TAG, "playback error: ${error.errorCodeName} ($detail)")
            onError(error)
        }

        override fun onPlaybackStateChanged(playbackState: Int) = onPlaybackState(playbackState)

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            AppLog.i(TAG, if (isPlaying) "playing" else "not playing (${stateName(player.playbackState)})")
            if (isPlaying) player.currentMediaItem?.let { recovery.onPlaying(itemKey(it)) }
        }
    }

    private fun songOf(item: MediaItem): Song? {
        songs.get(item.mediaId)?.let { return it }
        return SongItems.song(item)?.also { songs.put(item.mediaId, it) }
    }

    private fun currentSong(): Song? = player.currentMediaItem?.let(::songOf)

    /** The current index and the next N in play order (shuffle, repeat), without repeats. */
    private fun playWindow(count: Int): List<Int> {
        val tl = player.currentTimeline
        if (tl.isEmpty) return emptyList()
        val repeat = if (player.repeatMode == Player.REPEAT_MODE_ONE) Player.REPEAT_MODE_OFF else player.repeatMode
        val out = mutableListOf(player.currentMediaItemIndex)
        var i = out[0]
        repeat(count) {
            i = tl.getNextWindowIndex(i, repeat, player.shuffleModeEnabled)
            if (i == C.INDEX_UNSET || i in out) return out
            out += i
        }
        return out
    }

    private fun updatePrefetch(evenPaused: Boolean = false) {
        if (player.mediaItemCount == 0) {
            prefetcher.stop()
            Downloads.setPlaying(emptySet())
            return
        }
        // The current and the next song's downloads are not deleted while the player reads them.
        Downloads.setPlaying(playWindow(1).mapTo(HashSet()) { player.getMediaItemAt(it).mediaId })
        // Only while playing is wanted: a restored, paused queue costs nothing until play (or until
        // a seek waits for the whole track, [evenPaused]).
        if (!player.playWhenReady && !evenPaused) return
        val items = playWindow(settings.prefetchCount).map { player.getMediaItemAt(it) }
        val ids = items.map { it.mediaId }
        source.retainOnly(ids.toSet())
        items.forEach { item -> songOf(item)?.let(source::learn) }
        prefetcher.update(ids)
    }

    private fun updateReplayGain() {
        val tl = player.currentTimeline
        val song = currentSong()
        if (tl.isEmpty || song == null) {
            replayGainVolume = 1f
            applyVolume()
            return
        }
        val idx = player.currentMediaItemIndex
        val shuffle = player.shuffleModeEnabled
        val prev = tl.getPreviousWindowIndex(idx, Player.REPEAT_MODE_OFF, shuffle).takeIf { it != C.INDEX_UNSET }
        val next = tl.getNextWindowIndex(idx, Player.REPEAT_MODE_OFF, shuffle).takeIf { it != C.INDEX_UNSET }
        val around = listOfNotNull(prev, idx, next).mapNotNull { songOf(player.getMediaItemAt(it)) }.map(SongItems::queueEntry)
        val inOrder = ReplayGainMath.albumInOrder(around, if (prev != null) 1 else 0, shuffle)
        val mode = settings.replayGain
        val v = ReplayGainMath.volumeFor(mode, song.replayGain, inOrder)
        if (v != replayGainVolume) {
            val how = if (mode == ReplayGainMode.Album) (if (inOrder) " (album in order)" else " (not in album order: track gain)") else ""
            AppLog.i(TAG, "replaygain ${mode.name.lowercase()}$how: song ${song.id} volume $v")
        }
        replayGainVolume = v
        applyVolume()
    }

    private fun applyVolume() {
        player.volume = replayGainVolume * fadeVolume
    }

    private fun updateScrobble() {
        val item = player.currentMediaItem
        scrobbler.onItem(
            trackKey = item?.let(::itemKey),
            songId = item?.mediaId,
            durationMs = item?.let(::songOf)?.duration?.let { it * 1000L },
            playWhenReady = player.playWhenReady,
            isPlaying = player.isPlaying,
            wallClockMs = System.currentTimeMillis(),
        )
    }

    // ---- Favourite button -----------------------------------------------------------------

    /** The heart: a change made here (kept on disk, [Library.favourites]) or the server's `starred`. */
    private fun isFavourite(song: Song): Boolean = Library.isFavourite(song.id, song.starred)

    private fun updateButtons() {
        val fav = currentSong()?.let(::isFavourite) ?: false
        if (!extras.containsKey(PlaybackCommands.EXTRA_FAVOURITE) || extras.getBoolean(PlaybackCommands.EXTRA_FAVOURITE) != fav) {
            extras.putBoolean(PlaybackCommands.EXTRA_FAVOURITE, fav)
            session.setSessionExtras(Bundle(extras))
        }
        val button = CommandButton.Builder(if (fav) CommandButton.ICON_HEART_FILLED else CommandButton.ICON_HEART_UNFILLED)
            .setDisplayName(if (fav) "Remove from favourites" else "Add to favourites")
            .setSessionCommand(PlaybackCommands.favourite)
            .build()
        session.setMediaButtonPreferences(ImmutableList.of(button))
    }

    /**
     * Stars or unstars the current song. The result says what happened: success, or
     * [SessionError.ERROR_PERMISSION_DENIED] when a read-only build refused it, so the UI can say so.
     * It goes through [Library.setFavourite], so it is kept on disk and, offline, queued.
     */
    private fun toggleFavourite(): ListenableFuture<SessionResult> {
        val song = currentSong() ?: return Futures.immediateFuture(SessionResult(SessionError.ERROR_BAD_VALUE))
        val client = AppSession.current ?: return Futures.immediateFuture(SessionResult(SessionError.ERROR_SESSION_DISCONNECTED))
        val was = isFavourite(song)
        val result = SettableFuture.create<SessionResult>()
        scope.launch {
            val ok = Library.setFavourite(Item.OfSong(song), !was, report = false)
            updateButtons()
            result.set(
                when {
                    ok -> SessionResult(SessionResult.RESULT_SUCCESS)
                    BuildConfig.READ_ONLY && client.readOnly -> SessionResult(SessionError.ERROR_PERMISSION_DENIED)
                    else -> SessionResult(SessionError.ERROR_IO)
                },
            )
        }
        return result
    }

    // ---- Sleep timer ----------------------------------------------------------------------

    private fun onSleepModeChanged(mode: SleepMode) {
        player.pauseAtEndOfMediaItems = mode == SleepMode.EndOfTrack
        extras.remove(PlaybackCommands.EXTRA_SLEEP_ENDS_AT_WALL)
        when (mode) {
            SleepMode.Off -> extras.putString(PlaybackCommands.EXTRA_SLEEP_MODE, "off")
            SleepMode.EndOfTrack -> extras.putString(PlaybackCommands.EXTRA_SLEEP_MODE, "track")
            is SleepMode.At -> {
                extras.putString(PlaybackCommands.EXTRA_SLEEP_MODE, "at")
                extras.putLong(
                    PlaybackCommands.EXTRA_SLEEP_ENDS_AT_WALL,
                    System.currentTimeMillis() + (mode.endsAtMs - SystemClock.elapsedRealtime()),
                )
            }
        }
        AppLog.i(TAG, "sleep timer: $mode")
        session.setSessionExtras(Bundle(extras))
    }

    private fun setSleep(minutes: Int) {
        when {
            minutes > 0 -> sleepTimer.setMinutes(minutes)
            minutes == PlaybackCommands.SLEEP_END_OF_TRACK -> sleepTimer.setEndOfTrack()
            else -> sleepTimer.cancel()
        }
    }

    // ---- Queue editing -----------------------------------------------------------------

    /** The shuffle play order over the timeline (followed while shuffle is on, kept while it is off). */
    private fun shuffleOrder(): List<Int> {
        val tl = player.currentTimeline
        if (tl.isEmpty) return emptyList()
        val out = ArrayList<Int>(tl.windowCount)
        var i = tl.getFirstWindowIndex(true)
        while (i != C.INDEX_UNSET && out.size < tl.windowCount) {
            out += i
            i = tl.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, true)
        }
        return out
    }

    private fun setPlayOrder(order: List<Int>) {
        if (!QueueOrder.isValid(order, player.mediaItemCount)) return
        player.setShuffleOrder(ShuffleOrder.DefaultShuffleOrder(order.toIntArray(), System.nanoTime()))
    }

    /**
     * "Play next" and "Add to queue". Play next goes into the timeline right after the current
     * item (next with shuffle off) and right after it in the shuffle order (next with shuffle on);
     * Add to queue goes last in both. Media3's own shuffle order would scatter the new items.
     */
    private fun enqueue(songs: List<Song>, next: Boolean) {
        if (songs.isEmpty()) return
        val items = songs.map(SongItems::toMediaItem)
        if (player.mediaItemCount == 0) {
            player.setMediaItems(items)
            player.prepare()
            player.play()
            AppLog.i(TAG, "queue: was empty, playing ${items.size} songs")
            return
        }
        val order = shuffleOrder()
        val current = player.currentMediaItemIndex
        val count = player.mediaItemCount
        if (next) {
            player.addMediaItems(QueueOrder.timelineIndexForPlayNext(current, count), items)
            setPlayOrder(QueueOrder.orderAfterPlayNext(order, current, items.size))
        } else {
            player.addMediaItems(count, items)
            setPlayOrder(QueueOrder.orderAfterAppend(order, items.size))
        }
        val how = if (next) "next" else "added last"
        val shuffled = if (player.shuffleModeEnabled) " (shuffle)" else ""
        AppLog.i(TAG, "queue: ${items.size} songs $how$shuffled, now ${player.mediaItemCount}")
        logQueue()
    }

    /** Moves the entry at play position [from] to [to]: a timeline move, or a new shuffle order under shuffle. */
    private fun moveInQueue(from: Int, to: Int) {
        val n = player.mediaItemCount
        if (from !in 0 until n || to !in 0 until n || from == to) return
        if (player.shuffleModeEnabled) setPlayOrder(QueueOrder.move(shuffleOrder(), from, to)) else player.moveMediaItem(from, to)
        AppLog.i(TAG, "queue: moved $from -> $to" + if (player.shuffleModeEnabled) " (shuffle order)" else "")
        logQueue()
    }

    private fun removeFromQueue(index: Int) {
        if (index !in 0 until player.mediaItemCount || index == player.currentMediaItemIndex) return
        player.removeMediaItem(index) // the shuffle order keeps the others in order
        AppLog.i(TAG, "queue: removed $index, now ${player.mediaItemCount}")
        logQueue()
    }

    private fun clearUpcoming() {
        val current = player.currentMediaItemIndex
        val n = player.mediaItemCount
        if (n == 0) return
        if (player.shuffleModeEnabled) {
            QueueOrder.upcoming(shuffleOrder(), current).sortedDescending().forEach { player.removeMediaItem(it) }
        } else if (current + 1 < n) {
            player.removeMediaItems(current + 1, n)
        }
        AppLog.i(TAG, "queue: cleared upcoming, now ${player.mediaItemCount}")
        logQueue()
    }

    /** The queue in play order as song ids, the current one starred (debug builds; ids only, no names). */
    private fun logQueue() {
        if (!BuildConfig.DEBUG) return
        val shuffle = player.shuffleModeEnabled
        val order = if (shuffle) shuffleOrder() else QueueOrder.identity(player.mediaItemCount)
        val ids = order.map { player.getMediaItemAt(it).mediaId + if (it == player.currentMediaItemIndex) "*" else "" }
        AppLog.i(TAG, "queue order (" + (if (shuffle) "shuffle" else "in order") + "): " + ids.joinToString(" "))
    }

    // ---- Saved queue -------------------------------------------------------------------

    private fun snapshot(): SavedQueue? {
        if (player.mediaItemCount == 0) return null
        val list = (0 until player.mediaItemCount).mapNotNull { i ->
            songOf(player.getMediaItemAt(i))?.let { s -> Library.favourites.value[s.id]?.let { s.copy(starred = if (it) "1" else null) } ?: s }
        }
        return SavedQueue(
            songs = list,
            index = player.currentMediaItemIndex,
            positionMs = player.currentPosition.coerceAtLeast(0),
            shuffle = player.shuffleModeEnabled,
            repeatMode = player.repeatMode,
            savedAtMs = System.currentTimeMillis(),
            shuffleOrder = shuffleOrder(),
        )
    }

    private fun save() {
        val seq = queueStore.nextSeq()
        val q = snapshot()
        saveJob?.cancel()
        saveJob = scope.launch(io) { if (q == null) queueStore.clear(seq) else queueStore.save(q, seq) }
    }

    private fun saveNow() {
        val seq = queueStore.nextSeq()
        val q = snapshot()
        if (q == null) queueStore.clear(seq) else queueStore.save(q, seq)
    }

    private fun startPeriodicSave() {
        if (periodicSave?.isActive == true) return
        periodicSave = scope.launch {
            while (true) {
                delay(SAVE_EVERY_MS)
                save()
            }
        }
    }

    private fun stopPeriodicSave() {
        periodicSave?.cancel()
        periodicSave = null
    }

    private fun itemsOf(saved: SavedQueue): MediaSession.MediaItemsWithStartPosition =
        MediaSession.MediaItemsWithStartPosition(saved.songs.map(SongItems::toMediaItem), saved.index, saved.positionMs)

    /** On start: the saved queue goes back in, paused at its position, not prepared (no network). */
    private fun restoreIfEmpty(saved: SavedQueue?) {
        if (saved == null || player.mediaItemCount > 0) return
        val items = itemsOf(saved)
        player.setMediaItems(items.mediaItems, items.startIndex, items.startPositionMs)
        player.shuffleModeEnabled = saved.shuffle
        player.repeatMode = saved.repeatMode
        saved.validShuffleOrder?.let(::setPlayOrder)
        player.playWhenReady = false
        AppLog.i(TAG, "restored ${saved.songs.size} songs at ${saved.index}, ${saved.positionMs} ms, paused")
    }

    // ---- Session -----------------------------------------------------------------------

    private val sessionCallback = object : MediaLibrarySession.Callback {
        override fun onConnect(session: MediaSession, controller: MediaSession.ControllerInfo): MediaSession.ConnectionResult {
            // Any app can connect: only the app itself and the controllers the system vouches for
            // (the notification, Auto, Bluetooth, Assistant, Wear) get the browse tree, the custom
            // commands (the heart writes to the server) and the covers; others only play and pause.
            val grant = ControllerTrust.grantFor(
                sameApp = controller.uid == android.os.Process.myUid(),
                trustedBySystem = controller.isTrusted,
                mediaNotification = session.isMediaNotificationController(controller),
                autoOrAutomotive = session.isAutoCompanionController(controller) ||
                    session.isAutomotiveController(controller),
                packageName = controller.packageName,
            )
            if (controller.packageName != packageName) {
                AppLog.i(TAG, "controller connected: ${controller.packageName} (${if (grant == ControllerTrust.Grant.Full) "trusted" else "playback only"})")
            }
            if (grant == ControllerTrust.Grant.PlayerOnly) {
                return MediaSession.ConnectionResult.AcceptedResultBuilder(session)
                    .setAvailableSessionCommands(MediaSession.ConnectionResult.DEFAULT_SESSION_COMMANDS)
                    .build()
            }
            // Android Auto, the system's media controls: they load artwork URIs themselves.
            if (controller.packageName != packageName) Covers.grantTo(this@PlaybackService, controller.packageName)
            val commands = MediaSession.ConnectionResult.DEFAULT_SESSION_AND_LIBRARY_COMMANDS.buildUpon()
                .add(PlaybackCommands.favourite)
                .add(PlaybackCommands.sleepTimer)
                .add(PlaybackCommands.enqueue)
                .add(PlaybackCommands.queueMove)
                .add(PlaybackCommands.queueRemove)
                .add(PlaybackCommands.queueClearUpcoming)
                .build()
            return MediaSession.ConnectionResult.AcceptedResultBuilder(session)
                .setAvailableSessionCommands(commands)
                .build()
        }

        override fun onCustomCommand(
            session: MediaSession,
            controller: MediaSession.ControllerInfo,
            customCommand: SessionCommand,
            args: Bundle,
        ): ListenableFuture<SessionResult> {
            when (customCommand.customAction) {
                PlaybackCommands.FAVOURITE -> return toggleFavourite()
                PlaybackCommands.SLEEP_TIMER -> setSleep(args.getInt(PlaybackCommands.ARG_MINUTES, PlaybackCommands.SLEEP_OFF))
                PlaybackCommands.ENQUEUE -> enqueue(
                    SongItems.decodeSongs(args.getString(PlaybackCommands.ARG_SONGS)),
                    args.getBoolean(PlaybackCommands.ARG_NEXT),
                )
                PlaybackCommands.QUEUE_MOVE -> moveInQueue(args.getInt(PlaybackCommands.ARG_FROM), args.getInt(PlaybackCommands.ARG_TO))
                PlaybackCommands.QUEUE_REMOVE -> removeFromQueue(args.getInt(PlaybackCommands.ARG_INDEX, -1))
                PlaybackCommands.QUEUE_CLEAR_UPCOMING -> clearUpcoming()
                else -> return Futures.immediateFuture(SessionResult(SessionError.ERROR_NOT_SUPPORTED))
            }
            return Futures.immediateFuture(SessionResult(SessionResult.RESULT_SUCCESS))
        }

        /** Items added to the queue: songs as they are; a browse item (Auto) as its songs, a track alone. */
        override fun onAddMediaItems(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
            mediaItems: MutableList<MediaItem>,
        ): ListenableFuture<MutableList<MediaItem>> {
            if (mediaItems.none { BrowseId.parse(it.mediaId) != null }) {
                return Futures.immediateFuture(mediaItems.mapNotNull(SongItems::resolve).toMutableList())
            }
            return future {
                mediaItems.flatMap { item ->
                    when (val node = BrowseId.parse(item.mediaId)) {
                        null -> listOfNotNull(SongItems.resolve(item))
                        is BrowseId.Track -> auto.queueFor(item.mediaId)?.let { (songs, i) -> listOfNotNull(songs.getOrNull(i)?.takeIf { it.id == node.songId }?.let(SongItems::toMediaItem)) }.orEmpty()
                        else -> auto.queueFor(item.mediaId)?.first.orEmpty().map(SongItems::toMediaItem)
                    }
                }.toMutableList()
            }
        }

        /**
         * A new queue. From the app: its songs, as given. From Android Auto: one browse item (an album
         * or playlist plays from the top, a track plays its list from it), or a spoken search (no id,
         * a query: its songs; no query, "play some music": the saved queue).
         */
        override fun onSetMediaItems(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
            mediaItems: MutableList<MediaItem>,
            startIndex: Int,
            startPositionMs: Long,
        ): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val single = mediaItems.singleOrNull()
            if (single != null && BrowseId.parse(single.mediaId) != null) {
                return future {
                    val (songs, start) = auto.queueFor(single.mediaId) ?: throw UnsupportedOperationException("Nothing to play")
                    AppLog.i(TAG, "browse play: ${songs.size} songs from $start (${controller.packageName})")
                    MediaSession.MediaItemsWithStartPosition(songs.map(SongItems::toMediaItem), start, 0)
                }
            }
            val query = single?.requestMetadata?.searchQuery
            if (single != null && single.mediaId.isEmpty() && query != null) {
                return future {
                    if (query.isBlank()) {
                        val saved = withContext(Dispatchers.IO) { queueStore.load() } ?: throw UnsupportedOperationException("No saved queue")
                        AppLog.i(TAG, "voice play without a query: the saved queue")
                        itemsOf(saved)
                    } else {
                        val songs = auto.queueForSearch(query) ?: throw UnsupportedOperationException("Nothing found")
                        AppLog.i(TAG, "voice search play: ${songs.size} songs")
                        MediaSession.MediaItemsWithStartPosition(songs.map(SongItems::toMediaItem), 0, 0)
                    }
                }
            }
            return Futures.immediateFuture(
                MediaSession.MediaItemsWithStartPosition(mediaItems.mapNotNull(SongItems::resolve), startIndex, startPositionMs),
            )
        }

        /** Bluetooth or the system's media resumption card, with nothing in the queue. */
        override fun onPlaybackResumption(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
        ): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val result = SettableFuture.create<MediaSession.MediaItemsWithStartPosition>()
            scope.launch {
                val saved = withContext(Dispatchers.IO) {
                    AppSession.ensureLoaded(this@PlaybackService)
                    queueStore.load()
                }
                if (saved == null) {
                    result.setException(UnsupportedOperationException("No saved queue"))
                } else {
                    AppLog.i(TAG, "playback resumption: ${saved.songs.size} songs at ${saved.index}, ${saved.positionMs} ms")
                    player.shuffleModeEnabled = saved.shuffle
                    player.repeatMode = saved.repeatMode
                    pendingShuffleOrder = saved.validShuffleOrder
                    result.set(itemsOf(saved))
                }
            }
            return result
        }

        override fun onGetLibraryRoot(
            session: MediaLibrarySession,
            browser: MediaSession.ControllerInfo,
            params: LibraryParams?,
        ): ListenableFuture<LibraryResult<MediaItem>> {
            // Content style hints for the whole tree (nodes override them for their children).
            val extras = Bundle(params?.extras ?: Bundle.EMPTY).apply { putAll(auto.rootExtras()) }
            val rootParams = LibraryParams.Builder()
                .setOffline(params?.isOffline ?: false)
                .setSuggested(params?.isSuggested ?: false)
                .setRecent(params?.isRecent ?: false)
                .setExtras(extras)
                .build()
            return Futures.immediateFuture(LibraryResult.ofItem(auto.root(), rootParams))
        }

        override fun onGetChildren(
            session: MediaLibrarySession,
            browser: MediaSession.ControllerInfo,
            parentId: String,
            page: Int,
            pageSize: Int,
            params: LibraryParams?,
        ): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> = future {
            val items = auto.page(parentId, page, pageSize)
            if (items == null) LibraryResult.ofError(SessionError.ERROR_BAD_VALUE)
            else LibraryResult.ofItemList(ImmutableList.copyOf(items), params)
        }

        override fun onGetItem(
            session: MediaLibrarySession,
            browser: MediaSession.ControllerInfo,
            mediaId: String,
        ): ListenableFuture<LibraryResult<MediaItem>> = future {
            auto.item(mediaId)?.let { LibraryResult.ofItem(it, null) } ?: LibraryResult.ofError(SessionError.ERROR_BAD_VALUE)
        }

        override fun onSearch(
            session: MediaLibrarySession,
            browser: MediaSession.ControllerInfo,
            query: String,
            params: LibraryParams?,
        ): ListenableFuture<LibraryResult<Void>> {
            scope.launch {
                val n = catching { auto.search(query).size }.getOrDefault(0)
                session.notifySearchResultChanged(browser, query, n, params)
            }
            return Futures.immediateFuture(LibraryResult.ofVoid())
        }

        override fun onGetSearchResult(
            session: MediaLibrarySession,
            browser: MediaSession.ControllerInfo,
            query: String,
            page: Int,
            pageSize: Int,
            params: LibraryParams?,
        ): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> = future {
            LibraryResult.ofItemList(ImmutableList.copyOf(BrowsePaging.page(auto.search(query), page, pageSize)), params)
        }
    }

    /** A suspend block as the future Media3 wants, run on the main scope. */
    private fun <T> future(block: suspend () -> T): ListenableFuture<T> {
        val f = SettableFuture.create<T>()
        scope.launch {
            try {
                f.set(block())
            } catch (e: kotlinx.coroutines.CancellationException) {
                f.cancel(false)
                throw e
            } catch (e: Exception) {
                AppLog.i(TAG, "request failed: ${e.javaClass.simpleName}")
                f.setException(e)
            }
        }
        return f
    }

    private fun transitionReason(reason: Int) = when (reason) {
        Player.MEDIA_ITEM_TRANSITION_REASON_AUTO -> "auto"
        Player.MEDIA_ITEM_TRANSITION_REASON_SEEK -> "seek"
        Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT -> "repeat"
        else -> "queue changed"
    }

    private companion object {
        const val TAG = "MarqueePlayback"
        const val SAVE_EVERY_MS = 30_000L
        val NETWORK_IO_ERRORS = setOf(
            PlaybackException.ERROR_CODE_IO_UNSPECIFIED,
            PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED,
            PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_TIMEOUT,
        )
    }
}
