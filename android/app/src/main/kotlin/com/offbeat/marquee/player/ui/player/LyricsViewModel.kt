package com.offbeat.marquee.player.ui.player

import androidx.compose.runtime.Immutable
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.offbeat.marquee.core.lyrics.LrclibError
import com.offbeat.marquee.core.lyrics.Lyrics
import com.offbeat.marquee.core.lyrics.LyricsAnswer
import com.offbeat.marquee.core.lyrics.LyricsLookupError
import com.offbeat.marquee.core.lyrics.LyricsResolver
import com.offbeat.marquee.core.lyrics.LyricsTrack
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.data.LyricsRepo
import com.offbeat.marquee.player.data.OnlineLyrics
import com.offbeat.marquee.player.session.errorMessage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** What the lyrics view shows. */
sealed interface LyricsUi {
    data object Loading : LyricsUi
    data class Shown(val lyrics: Lyrics) : LyricsUi
    data object Instrumental : LyricsUi

    /** [searchReason] says why "Search online" is off (null: it can be used). */
    data class NotFound(val searchReason: String?) : LyricsUi
    data class Failed(val message: String) : LyricsUi
}

@Immutable
data class LyricsViewState(val songId: String = "", val ui: LyricsUi = LyricsUi.Loading, val refreshing: Boolean = false)

/**
 * The lyrics view's data, for the activity (so a rotation keeps it). It works only while the view
 * is visible and the app is in the foreground: [show] is called then, [hide] cancels everything
 * when the view closes or the app goes to the background. Per track it asks the cache, then (after
 * a short pause, so skipping through tracks sends nothing) the sources; once the current track's
 * lyrics are in, it looks the next track up once, ahead of time.
 */
class LyricsViewModel : ViewModel() {
    private val _state = MutableStateFlow(LyricsViewState())
    val state: StateFlow<LyricsViewState> = _state.asStateFlow()

    private var job: Job? = null
    private var current: LyricsTrack? = null
    private var next: LyricsTrack? = null
    /** Tracks looked up ahead this session (once each, found or not). */
    private val prefetched = HashSet<String>()

    fun show(track: LyricsTrack, nextTrack: LyricsTrack?) {
        next = nextTrack
        if (track == current) {
            // Still working on it (it reads [next] when it gets there).
            if (job?.isActive == true) return
            val s = _state.value
            if (s.songId == track.songId && s.ui !is LyricsUi.Loading && s.ui !is LyricsUi.Failed) {
                job = viewModelScope.launch { prefetchNext() }
                return
            }
        }
        current = track
        job?.cancel()
        job = viewModelScope.launch {
            load(track, LyricsResolver.Mode.Normal, debounce = true)
            prefetchNext()
        }
    }

    /** The view closed, or the app went to the background: nothing more is asked. */
    fun hide() {
        job?.cancel()
        job = null
        // Left half-way, the next show starts over.
        if (_state.value.ui is LyricsUi.Loading || _state.value.refreshing) {
            _state.update { it.copy(songId = "", refreshing = false) }
        }
    }

    /** Pull to refresh: the server is asked again. */
    fun refresh() = rerun(LyricsResolver.Mode.Refresh, keepShown = true)

    fun searchOnline() = rerun(LyricsResolver.Mode.SearchOnline, keepShown = false)

    fun retry() = rerun(LyricsResolver.Mode.Normal, keepShown = false)

    private fun rerun(mode: LyricsResolver.Mode, keepShown: Boolean) {
        val track = current ?: return
        job?.cancel()
        job = viewModelScope.launch {
            if (keepShown) _state.update { it.copy(refreshing = true) }
            load(track, mode, debounce = false, keepShown = keepShown)
        }
    }

    private suspend fun load(track: LyricsTrack, mode: LyricsResolver.Mode, debounce: Boolean, keepShown: Boolean = false) {
        if (mode == LyricsResolver.Mode.Normal) {
            LyricsRepo.fromCache(track)?.let {
                _state.value = LyricsViewState(track.songId, uiOf(track, it))
                return
            }
        }
        if (!keepShown) _state.value = LyricsViewState(track.songId, LyricsUi.Loading)
        if (debounce) delay(DEBOUNCE_MS)
        val ui = try {
            uiOf(track, LyricsRepo.resolve(track, mode))
        } catch (e: CancellationException) {
            throw e
        } catch (e: LyricsLookupError) {
            if (keepShown && _state.value.ui is LyricsUi.Shown) {
                _state.update { it.copy(refreshing = false) }
                return
            }
            LyricsUi.Failed(messageOf(e))
        }
        _state.value = LyricsViewState(track.songId, ui)
    }

    private suspend fun prefetchNext() {
        val n = next ?: return
        if (n.songId == current?.songId || !prefetched.add(n.songId)) return
        if (LyricsRepo.fromCache(n) != null) return
        try {
            LyricsRepo.resolve(n)
        } catch (e: LyricsLookupError) {
            // Ahead of time only: it is asked again when that track plays.
            prefetched.remove(n.songId)
        }
    }

    private fun uiOf(track: LyricsTrack, a: LyricsAnswer): LyricsUi = when (a) {
        is LyricsAnswer.Found -> LyricsUi.Shown(a.lyrics)
        LyricsAnswer.Instrumental -> LyricsUi.Instrumental
        LyricsAnswer.NotFound -> LyricsUi.NotFound(
            when (LyricsRepo.online()) {
                OnlineLyrics.OffInThisBuild -> "Online lookup is off in test builds, so LRCLIB isn't asked."
                OnlineLyrics.OffInSettings -> "Looking up lyrics online is off in Settings."
                OnlineLyrics.Available -> if (LyricsRepo.canSearchOnline(track)) null else "This track has no artist, title or length to look up."
            },
        )
    }

    private fun messageOf(e: LyricsLookupError): String {
        val cause = e.cause
        return when {
            e.busy -> "LRCLIB is busy right now. Try again in a minute."
            cause is SubsonicError -> errorMessage(cause)
            cause is LrclibError.Network -> "Couldn't reach LRCLIB. Check your connection."
            cause is LrclibError -> "LRCLIB didn't give a usable answer."
            else -> "Couldn't look up the lyrics."
        }
    }

    private companion object {
        /** Skipping through tracks with the view open sends nothing until one stays this long. */
        const val DEBOUNCE_MS = 450L
    }
}
