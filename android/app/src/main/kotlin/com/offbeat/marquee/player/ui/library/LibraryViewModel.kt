package com.offbeat.marquee.player.ui.library

import com.offbeat.marquee.player.diag.AppLog
import androidx.compose.runtime.Immutable
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.offbeat.marquee.core.library.AlbumPaging
import com.offbeat.marquee.core.library.AlbumSort
import com.offbeat.marquee.core.library.SortLetters
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.ArtistsIndex
import com.offbeat.marquee.core.subsonic.Genre
import com.offbeat.marquee.core.subsonic.Playlist
import com.offbeat.marquee.core.subsonic.Starred
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.session.errorMessage
import com.offbeat.marquee.player.settings.LibraryPrefs
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit

/**
 * The Albums tab under one sort. For every sort but Random the grid is [total] cells long from the
 * start and [pages] fills in as cells come near the screen; Random reads fresh pages one after
 * another into [random].
 */
@Immutable
data class AlbumsState(
    val sort: AlbumSort = AlbumSort.DEFAULT,
    val total: Int? = null,
    val pages: Map<Int, List<Album>> = emptyMap(),
    val random: List<Album> = emptyList(),
    val randomEnd: Boolean = false,
    val loading: Boolean = true,
    val error: String? = null,
) {
    /** Cells in the grid: the whole list once its size is known, else what has arrived. */
    val count: Int get() = if (sort.pagesByOffset) total ?: (pages[0]?.size ?: 0) else random.size

    fun albumAt(index: Int): Album? =
        if (sort.pagesByOffset) pages[AlbumPaging.pageOf(index)]?.getOrNull(index % AlbumPaging.PAGE_SIZE) else random.getOrNull(index)

    /** The nearest album that has arrived, for the bubble while its own page is on the way. */
    fun nearestAlbum(index: Int): Album? {
        albumAt(index)?.let { return it }
        for (d in 1..AlbumPaging.PAGE_SIZE * 2) {
            albumAt(index - d)?.let { return it }
            albumAt(index + d)?.let { return it }
        }
        return null
    }
}

/** A list tab: the last saved answer at once, then the server's. */
@Immutable
data class ListState<T>(val value: T? = null, val error: String? = null)

class LibraryViewModel : ViewModel() {
    private val _albums = MutableStateFlow(AlbumsState(LibraryPrefs.sort.value))
    val albums: StateFlow<AlbumsState> = _albums.asStateFlow()

    private val _artists = MutableStateFlow(ListState<ArtistsIndex>())
    val artists: StateFlow<ListState<ArtistsIndex>> = _artists.asStateFlow()

    private val _playlists = MutableStateFlow(ListState<List<Playlist>>())
    val playlists: StateFlow<ListState<List<Playlist>>> = _playlists.asStateFlow()

    private val _genres = MutableStateFlow(ListState<List<Genre>>())
    val genres: StateFlow<ListState<List<Genre>>> = _genres.asStateFlow()

    private val _starred = MutableStateFlow(ListState<Starred>())
    val starred: StateFlow<ListState<Starred>> = _starred.asStateFlow()

    /** The server's ignored articles (from getArtists), for the scrollers' letters. */
    private val _articles = MutableStateFlow(SortLetters.DEFAULT_ARTICLES)
    val articles: StateFlow<List<String>> = _articles.asStateFlow()

    /** A seed for the Random sort over favourites, so it keeps its order until reopened. */
    val randomSeed = System.nanoTime()

    private var albumsJob: Job? = null
    private val pageJobs = HashMap<Int, Job>()
    private val gate = Semaphore(2)
    private var randomJob: Job? = null
    private val started = HashSet<String>()

    private var shown: Pair<LibraryTab, Boolean>? = null

    init {
        startAlbums()
        // Back online: what is on screen refreshes once.
        viewModelScope.launch {
            com.offbeat.marquee.player.net.NetworkMonitor.cameBack.collect {
                if (_albums.value.error != null || _albums.value.count == 0) startAlbums()
                started.clear()
                shown?.let { (tab, fav) -> ensure(tab, fav) }
            }
        }
    }

    // ---- Albums --------------------------------------------------------------------------------

    fun setSort(sort: AlbumSort) {
        if (sort == _albums.value.sort) return
        LibraryPrefs.setSort(sort)
        startAlbums()
    }

    private fun startAlbums() {
        val sort = LibraryPrefs.sort.value
        albumsJob?.cancel()
        randomJob?.cancel()
        pageJobs.values.forEach { it.cancel() }
        pageJobs.clear()
        _albums.value = AlbumsState(sort)
        if (!sort.pagesByOffset) {
            loadMoreRandom()
            return
        }
        albumsJob = viewModelScope.launch {
            val cachedTotal = Library.cachedAlbumCount(sort)
            val cachedPage = Library.cachedAlbumPage(sort)
            _albums.update { s -> s.copy(total = cachedTotal, pages = cachedPage?.let { mapOf(0 to it) } ?: s.pages) }
            // The first page is always refreshed; the others come as they are needed.
            launchPage(sort, 0)
            try {
                val total = Library.albumCount(sort)
                _albums.update { if (it.sort == sort) it.copy(total = total, loading = false) else it }
            } catch (e: SubsonicError) {
                _albums.update { it.copy(loading = false, error = if (it.pages.isEmpty()) errorMessage(e) else null) }
            }
        }
    }

    /** The grid shows cells [first]..[last]: fetch the pages there and a little ahead, drop the rest. */
    fun onVisible(first: Int, last: Int) {
        val s = _albums.value
        if (!s.sort.pagesByOffset) {
            if (last >= s.random.size - 12) loadMoreRandom()
            return
        }
        val total = s.total ?: return
        val wanted = AlbumPaging.wantedPages(first, last, total)
        // Pages the screen has moved away from (a fast-scroller drag passing through) are not worth finishing.
        pageJobs.entries.removeAll { (page, job) ->
            val stale = page !in wanted && job.isActive
            if (stale) job.cancel()
            stale || !job.isActive
        }
        for (page in wanted) if (page !in s.pages && pageJobs[page]?.isActive != true) launchPage(s.sort, page)
    }

    private fun launchPage(sort: AlbumSort, page: Int) {
        pageJobs[page] = viewModelScope.launch {
            try {
                val list = gate.withPermit { Library.fetchAlbumPage(sort, page) }
                _albums.update { s -> if (s.sort == sort) s.copy(pages = s.pages + (page to list), error = null, loading = s.total == null) else s }
            } catch (e: CancellationException) {
                throw e
            } catch (e: SubsonicError) {
                AppLog.w("MarqueeLibrary", "albums ${sort.key}: page $page failed (${e.javaClass.simpleName})")
                _albums.update { s -> if (s.sort == sort && s.pages.isEmpty()) s.copy(error = errorMessage(e), loading = false) else s }
            }
        }
    }

    private fun loadMoreRandom() {
        val s = _albums.value
        if (s.randomEnd || randomJob?.isActive == true) return
        randomJob = viewModelScope.launch {
            _albums.update { it.copy(loading = true, error = null) }
            try {
                val page = Library.fetchAlbumPage(AlbumSort.Random, 0)
                _albums.update { cur ->
                    val seen = cur.random.mapTo(HashSet()) { it.id }
                    val fresh = page.filter { seen.add(it.id) }
                    // A random page that brings almost nothing new means the library is used up.
                    cur.copy(random = cur.random + fresh, loading = false, randomEnd = fresh.size < 5)
                }
            } catch (e: SubsonicError) {
                _albums.update { it.copy(loading = false, error = errorMessage(e)) }
            }
        }
    }

    fun retryAlbums() = startAlbums()

    // ---- The other tabs: loaded when first shown -------------------------------------------------

    fun ensure(tab: LibraryTab, favourites: Boolean) {
        shown = tab to favourites
        when (tab) {
            LibraryTab.Albums -> if (favourites) ensureStarred()
            LibraryTab.Artists -> if (favourites) ensureStarred() else ensureArtists()
            LibraryTab.Playlists -> ensurePlaylists()
            LibraryTab.Genres -> ensureGenres()
        }
    }

    private fun once(key: String, block: suspend () -> Unit) {
        if (!started.add(key)) return
        viewModelScope.launch { block() }
    }

    private fun ensureArtists() = once("artists") {
        Library.cachedArtists()?.let { a -> _artists.update { it.copy(value = a) }; _articles.value = SortLetters.articles(a.ignoredArticles) }
        try {
            val a = Library.fetchArtists()
            _artists.value = ListState(a)
            _articles.value = SortLetters.articles(a.ignoredArticles)
        } catch (e: SubsonicError) {
            _artists.update { it.copy(error = errorMessage(e)) }
            started.remove("artists")
        }
    }

    private fun ensurePlaylists() = once("playlists") {
        Library.cachedPlaylists()?.let { p -> _playlists.update { it.copy(value = p) } }
        try {
            _playlists.value = ListState(Library.fetchPlaylists())
        } catch (e: SubsonicError) {
            _playlists.update { it.copy(error = errorMessage(e)) }
            started.remove("playlists")
        }
    }

    private fun ensureGenres() = once("genres") {
        Library.cachedGenres()?.let { g -> _genres.update { it.copy(value = g) } }
        try {
            _genres.value = ListState(Library.fetchGenres())
        } catch (e: SubsonicError) {
            _genres.update { it.copy(error = errorMessage(e)) }
            started.remove("genres")
        }
    }

    private fun ensureStarred() = once("starred") {
        Library.cachedStarred()?.let { s -> _starred.update { it.copy(value = s) } }
        try {
            _starred.value = ListState(Library.fetchStarred())
        } catch (e: SubsonicError) {
            _starred.update { it.copy(error = errorMessage(e)) }
            started.remove("starred")
        }
    }

    /** The articles are wanted by the album bubbles too: fetch the artists' index (small) once. */
    fun ensureArticles() = ensureArtists()

    fun retry(tab: LibraryTab, favourites: Boolean) {
        when (tab) {
            LibraryTab.Albums -> if (favourites) started.remove("starred") else return retryAlbums()
            LibraryTab.Artists -> started.remove(if (favourites) "starred" else "artists")
            LibraryTab.Playlists -> started.remove("playlists")
            LibraryTab.Genres -> started.remove("genres")
        }
        ensure(tab, favourites)
    }
}

enum class LibraryTab(val title: String) { Albums("Albums"), Artists("Artists"), Playlists("Playlists"), Genres("Genres") }
