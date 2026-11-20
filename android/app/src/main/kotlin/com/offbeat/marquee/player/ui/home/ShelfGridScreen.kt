package com.offbeat.marquee.player.ui.home

import androidx.compose.runtime.Immutable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Shuffle
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import com.offbeat.marquee.core.library.ShelfId
import com.offbeat.marquee.core.library.Shelves
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.session.errorMessage
import com.offbeat.marquee.player.ui.components.AlbumTile
import com.offbeat.marquee.player.ui.components.BackButton
import com.offbeat.marquee.player.ui.components.LoadingOrError
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

@Immutable
data class GridState(
    val albums: List<Album> = emptyList(),
    val loading: Boolean = true,
    val end: Boolean = false,
    val error: String? = null,
)

/**
 * A shelf's whole list, a page at a time (offset / size). The random shelf starts with the albums
 * the row showed and adds new random pages without repeats, until it is reshuffled.
 */
class ShelfGridViewModel(val shelf: ShelfId) : ViewModel() {
    private val _state = MutableStateFlow(GridState())
    val state: StateFlow<GridState> = _state.asStateFlow()
    private var job: Job? = null
    /** Paging waits for the first page, so the grid's scroll trigger can't ask for it twice. */
    private var started = false

    init {
        viewModelScope.launch {
            Library.cachedShelf(shelf)?.let { cached -> _state.update { it.copy(albums = cached) } }
            if (shelf.isRandom && _state.value.albums.isNotEmpty()) {
                _state.update { it.copy(loading = false) }
            } else {
                loadMore(reset = true)
            }
            started = true
        }
    }

    fun loadMore() {
        if (started) loadMore(reset = false)
    }

    private fun loadMore(reset: Boolean) {
        val s = _state.value
        if (!reset && (s.end || job?.isActive == true)) return
        job = viewModelScope.launch {
            _state.update { it.copy(loading = true, error = null) }
            try {
                val offset = if (reset || shelf.isRandom) 0 else s.albums.size
                val page = Library.fetchShelf(shelf, Shelves.PAGE_SIZE, offset)
                _state.update { cur ->
                    val base = if (reset) emptyList() else cur.albums
                    val seen = base.mapTo(HashSet()) { it.id }
                    val fresh = page.filter { seen.add(it.id) }
                    // A random page that brings almost nothing new means the library is used up.
                    val end = if (shelf.isRandom) fresh.size < 5 else page.size < Shelves.PAGE_SIZE
                    cur.copy(albums = base + fresh, loading = false, end = end)
                }
            } catch (e: SubsonicError) {
                _state.update { it.copy(loading = false, error = errorMessage(e)) }
            }
        }
    }

    fun reshuffle() {
        job?.cancel()
        _state.update { it.copy(albums = emptyList(), end = false) }
        loadMore(reset = true)
    }
}

@Composable
fun ShelfGridScreen(shelf: ShelfId, onBack: () -> Unit, onOpenAlbum: (String) -> Unit) {
    val vm: ShelfGridViewModel = viewModel(key = "shelf-${shelf.key}") { ShelfGridViewModel(shelf) }
    val state by vm.state.collectAsStateWithLifecycle()
    PagedAlbumGrid(state, vm::loadMore, onOpenAlbum) {
        Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            BackButton(onBack)
            Text(
                shelf.title,
                style = MaterialTheme.typography.headlineSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f).padding(start = 12.dp).semantics { heading() },
            )
            if (shelf.isRandom) {
                IconButton(onClick = vm::reshuffle, modifier = Modifier.size(48.dp)) {
                    Icon(Icons.Rounded.Shuffle, contentDescription = "Reshuffle", tint = Palette.current.textDim)
                }
            }
        }
    }
}

/**
 * A grid of albums read a page at a time from the start ("See all", a genre), the next page asked
 * for when the last rows come into view; [header] spans the top.
 */
@Composable
fun PagedAlbumGrid(state: GridState, onLoadMore: () -> Unit, onOpenAlbum: (String) -> Unit, header: @Composable () -> Unit) {
    val grid = rememberLazyGridState()
    // Page in when the last rows come into view. With the item count, so a page too small to push
    // the end out of view (a random page brings only the albums not seen yet) asks for the next one.
    LaunchedEffect(grid) {
        snapshotFlow {
            val info = grid.layoutInfo
            val last = info.visibleItemsInfo.lastOrNull()?.index ?: 0
            (last >= info.totalItemsCount - 12) to info.totalItemsCount
        }.distinctUntilChanged().collect { (near, _) -> if (near) onLoadMore() }
    }
    LazyVerticalGrid(
        columns = GridCells.Adaptive(minSize = AlbumCellMin),
        state = grid,
        contentPadding = PaddingValues(
            start = Tokens.PageMargin,
            end = Tokens.PageMargin,
            top = LocalScreenTop.current,
            bottom = 24.dp,
        ),
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
        modifier = Modifier.fillMaxSize(),
    ) {
        item(key = "top", span = { GridItemSpan(maxLineSpan) }, contentType = "top") { header() }
        items(state.albums, key = { it.id }, contentType = { "album" }) { a -> AlbumTile(a, onOpenAlbum) }
        if (state.albums.isEmpty() && (state.loading || state.error != null)) {
            item(key = "status", span = { GridItemSpan(maxLineSpan) }, contentType = "status") {
                LoadingOrError(state.error, onLoadMore, Modifier.padding(top = 48.dp))
            }
        } else if (state.loading) {
            item(key = "more", span = { GridItemSpan(maxLineSpan) }, contentType = "status") {
                Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp)
                }
            }
        } else if (state.error != null) {
            item(key = "error", span = { GridItemSpan(maxLineSpan) }, contentType = "status") {
                LoadingOrError(state.error, onLoadMore)
            }
        } else if (state.albums.isEmpty()) {
            item(key = "empty", span = { GridItemSpan(maxLineSpan) }, contentType = "status") {
                Text("Nothing here yet.", color = Palette.current.textDim)
            }
        }
    }
}

/** The smallest an album cell gets: two columns on a portrait phone, five on its side. */
val AlbumCellMin = 136.dp
