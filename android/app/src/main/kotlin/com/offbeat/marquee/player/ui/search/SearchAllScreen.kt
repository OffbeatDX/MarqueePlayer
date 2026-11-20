package com.offbeat.marquee.player.ui.search

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
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
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
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.Artist
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.session.errorMessage
import com.offbeat.marquee.player.ui.LocalPlayer
import com.offbeat.marquee.player.ui.components.AlbumTile
import com.offbeat.marquee.player.ui.components.ArtistRow
import com.offbeat.marquee.player.ui.components.BackButton
import com.offbeat.marquee.player.ui.components.LoadingOrError
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.components.SongLead
import com.offbeat.marquee.player.ui.components.SongRow
import com.offbeat.marquee.player.ui.home.AlbumCellMin
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import androidx.compose.foundation.layout.Column

@Immutable
data class SearchAllState(
    val artists: List<Artist> = emptyList(),
    val albums: List<Album> = emptyList(),
    val songs: List<Song> = emptyList(),
    val loading: Boolean = true,
    val end: Boolean = false,
    val error: String? = null,
)

/** Every result of one kind, [PAGE] at a time (`search3` offsets). */
class SearchAllViewModel(private val query: String, private val kind: String) : ViewModel() {
    private val _state = MutableStateFlow(SearchAllState())
    val state: StateFlow<SearchAllState> = _state.asStateFlow()
    private var job: Job? = null
    // Results read so far, repeats included: the next page's offset.
    private var fetched = 0

    init {
        loadMore()
    }

    fun loadMore() {
        val s = _state.value
        if (s.end || job?.isActive == true) return
        job = viewModelScope.launch {
            _state.update { it.copy(loading = true, error = null) }
            try {
                val r = when (kind) {
                    KIND_ARTISTS -> Library.search(query, PAGE, 0, 0, artistOffset = fetched)
                    KIND_ALBUMS -> Library.search(query, 0, PAGE, 0, albumOffset = fetched)
                    else -> Library.search(query, 0, 0, PAGE, songOffset = fetched)
                }
                val got = when (kind) {
                    KIND_ARTISTS -> r.artist.size
                    KIND_ALBUMS -> r.album.size
                    else -> r.song.size
                }
                fetched += got
                // Offset paging can repeat an entry across pages (a scan between two, ties in the
                // order), and the grid keys artists and albums by id: keep the first of each.
                _state.update {
                    val artistIds = it.artists.mapTo(HashSet()) { a -> a.id }
                    val albumIds = it.albums.mapTo(HashSet()) { a -> a.id }
                    it.copy(
                        artists = it.artists + r.artist.filter { a -> artistIds.add(a.id) },
                        albums = it.albums + r.album.filter { a -> albumIds.add(a.id) },
                        songs = it.songs + r.song,
                        loading = false,
                        end = got < PAGE,
                    )
                }
            } catch (e: SubsonicError) {
                _state.update { it.copy(loading = false, error = errorMessage(e)) }
            }
        }
    }

    private companion object {
        const val PAGE = 40
    }
}

/** "See all" from Search: one kind of result, paged as it scrolls. */
@Composable
fun SearchAllScreen(query: String, kind: String, onBack: () -> Unit, onOpenAlbum: (String) -> Unit, onOpenArtist: (String) -> Unit) {
    val vm: SearchAllViewModel = viewModel(key = "search-$kind-$query") { SearchAllViewModel(query, kind) }
    val state by vm.state.collectAsStateWithLifecycle()
    val player = LocalPlayer.current
    val grid = rememberLazyGridState()
    LaunchedEffect(grid) {
        // With the item count, so a page that leaves the end still in view asks for the next one.
        snapshotFlow {
            val info = grid.layoutInfo
            ((info.visibleItemsInfo.lastOrNull()?.index ?: 0) >= info.totalItemsCount - 8) to info.totalItemsCount
        }.distinctUntilChanged().collect { (near, _) -> if (near) vm.loadMore() }
    }
    val title = when (kind) {
        KIND_ARTISTS -> "Artists"
        KIND_ALBUMS -> "Albums"
        else -> "Songs"
    }
    LazyVerticalGrid(
        columns = GridCells.Adaptive(if (kind == KIND_ALBUMS) AlbumCellMin else 320.dp),
        state = grid,
        contentPadding = PaddingValues(
            start = if (kind == KIND_ALBUMS) Tokens.PageMargin else 0.dp,
            end = if (kind == KIND_ALBUMS) Tokens.PageMargin else 0.dp,
            top = LocalScreenTop.current,
            bottom = 24.dp,
        ),
        horizontalArrangement = Arrangement.spacedBy(if (kind == KIND_ALBUMS) 14.dp else 0.dp),
        verticalArrangement = Arrangement.spacedBy(if (kind == KIND_ALBUMS) 14.dp else 0.dp),
        modifier = Modifier.fillMaxSize(),
    ) {
        item(key = "top", span = { GridItemSpan(maxLineSpan) }) {
            Row(
                Modifier.fillMaxWidth().padding(top = 4.dp, start = if (kind == KIND_ALBUMS) 0.dp else 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                BackButton(onBack)
                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                    Text(title, style = MaterialTheme.typography.headlineSmall, modifier = Modifier.semantics { heading() })
                    Text("“$query”", style = MaterialTheme.typography.bodySmall, color = Palette.current.textDim, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
        when (kind) {
            KIND_ARTISTS -> items(state.artists, key = { it.id }) { ArtistRow(it, onOpenArtist) }
            KIND_ALBUMS -> items(state.albums, key = { it.id }) { AlbumTile(it, onOpenAlbum) }
            else -> itemsIndexed(state.songs, key = { i, s -> "s$i-${s.id}" }) { i, s ->
                SongRow(s, SongLead.Cover, onClick = { player.playSongs(state.songs, i) }, showAlbum = true)
            }
        }
        if (state.loading || state.error != null) {
            item(key = "status", span = { GridItemSpan(maxLineSpan) }) {
                if (state.error != null) {
                    LoadingOrError(state.error, vm::loadMore)
                } else {
                    Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp)
                    }
                }
            }
        }
    }
}
