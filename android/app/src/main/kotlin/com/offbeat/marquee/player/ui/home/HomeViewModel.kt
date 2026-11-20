package com.offbeat.marquee.player.ui.home

import androidx.compose.runtime.Immutable
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.offbeat.marquee.core.library.ShelfId
import com.offbeat.marquee.player.settings.UiSettings
import kotlinx.coroutines.flow.drop
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.session.errorMessage
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

@Immutable
data class HomeState(
    /** The shelves in display order, each with its albums (empty shelves are hidden by the screen). */
    val shelves: List<Pair<ShelfId, List<Album>>> = emptyList(),
    /** Nothing at all yet: neither the disk nor the server has answered. */
    val empty: Boolean = true,
    val refreshing: Boolean = false,
    val error: String? = null,
    /** The hero's featured album, an index into its shelf. */
    val hero: Int = 0,
)

/**
 * Home's shelves: the last good answers from disk at once, then a refresh from the server. The
 * random shelf keeps its order until reshuffled (a pull to refresh leaves it alone). The shelves
 * and their order are Settings > Home's ([UiSettings.shelves]), followed at once: a shelf shown
 * again comes from disk, then the server.
 */
class HomeViewModel : ViewModel() {
    private var order: List<ShelfId> = UiSettings.shelves.value.visible
    private val _state = MutableStateFlow(HomeState())
    val state: StateFlow<HomeState> = _state.asStateFlow()
    private val lists = HashMap<ShelfId, List<Album>>()
    private var refreshJob: Job? = null

    init {
        viewModelScope.launch {
            val cached = order.map { id -> async { id to Library.cachedShelf(id) } }.awaitAll()
            var any = false
            for ((id, list) in cached) if (list != null) {
                lists[id] = list
                any = true
            }
            if (any) publish()
            refresh(initial = true)
        }
        // Back online: the shelves refresh once.
        viewModelScope.launch { NetworkMonitor.cameBack.collect { refresh(initial = false) } }
        viewModelScope.launch {
            UiSettings.shelves.drop(1).collect { layout ->
                order = layout.visible
                publish()
                fetchMissing()
            }
        }
    }

    /** Shelves now shown that were never loaded: from disk, then the server. */
    private suspend fun fetchMissing() {
        val missing = order.filter { lists[it] == null }
        if (missing.isEmpty()) return
        for (id in missing) Library.cachedShelf(id)?.let { lists[id] = it }
        publish()
        for (id in missing) {
            try {
                lists[id] = Library.fetchShelf(id)
            } catch (e: SubsonicError) {
                // Offline or failing: what was on disk (if anything) stays.
            }
        }
        publish()
    }

    fun refresh() = refresh(initial = false)

    private fun refresh(initial: Boolean) {
        refreshJob?.cancel()
        _state.update { it.copy(refreshing = !initial || !it.empty, error = null) }
        refreshJob = viewModelScope.launch {
            val ids = order.filter { !it.isRandom || lists[it] == null }
            var failure: SubsonicError? = null
            ids.map { id ->
                async {
                    try {
                        lists[id] = Library.fetchShelf(id)
                    } catch (e: SubsonicError) {
                        failure = e
                    }
                }
            }.awaitAll()
            publish()
            _state.update { s ->
                // With saved shelves on screen, a network failure is just "offline".
                val message = failure?.let { e ->
                    when {
                        // Offline the banner says so; the saved shelves are shown without more.
                        e is SubsonicError.Network && !s.empty && !NetworkMonitor.online -> null
                        e is SubsonicError.Network && !s.empty -> "Can't reach the server. Showing what was saved."
                        else -> errorMessage(e)
                    }
                }
                s.copy(refreshing = false, error = message)
            }
        }
    }

    fun reshuffle() {
        viewModelScope.launch {
            try {
                lists[ShelfId.Random] = Library.fetchShelf(ShelfId.Random)
                publish()
            } catch (e: SubsonicError) {
                Library.post(errorMessage(e))
            }
        }
    }

    fun feature(index: Int) = _state.update { it.copy(hero = index) }

    private fun publish() {
        _state.update { s ->
            val shelves = order.mapNotNull { id -> lists[id]?.let { id to it } }
            val heroSize = lists[ShelfId.RecentlyAdded]?.size ?: 0
            s.copy(
                shelves = shelves,
                empty = shelves.isEmpty(),
                hero = if (s.hero < heroSize) s.hero else 0,
            )
        }
    }
}
