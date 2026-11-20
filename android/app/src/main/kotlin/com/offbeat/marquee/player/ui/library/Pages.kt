package com.offbeat.marquee.player.ui.library

import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.DownloadForOffline
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Shuffle
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
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
import com.offbeat.marquee.core.library.AlbumPaging
import com.offbeat.marquee.core.subsonic.Playlist
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.data.Item
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.core.offline.PinKind
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.ui.components.DownloadButton
import com.offbeat.marquee.player.session.errorMessage
import com.offbeat.marquee.player.ui.LocalPlayer
import com.offbeat.marquee.player.ui.actions.itemGestures
import com.offbeat.marquee.player.ui.actions.songsLabel
import com.offbeat.marquee.player.ui.components.AccentButton
import com.offbeat.marquee.player.ui.components.BackButton
import com.offbeat.marquee.player.ui.components.CollectionPage
import com.offbeat.marquee.player.ui.components.LoadingOrError
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.components.QuietButton
import com.offbeat.marquee.player.ui.components.SongLead
import com.offbeat.marquee.player.ui.components.SongRow
import com.offbeat.marquee.player.ui.home.GridState
import com.offbeat.marquee.player.ui.home.PagedAlbumGrid
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import com.offbeat.marquee.player.ui.actions.albumsLabel
import com.offbeat.marquee.player.ui.actions.songCountLabel

// ---- Playlist ------------------------------------------------------------------------------

@Immutable
data class PlaylistState(val playlist: Playlist? = null, val error: String? = null)

class PlaylistViewModel(private val id: String) : ViewModel() {
    private val _state = MutableStateFlow(PlaylistState())
    val state: StateFlow<PlaylistState> = _state.asStateFlow()

    init {
        load()
        viewModelScope.launch { NetworkMonitor.cameBack.collect { load() } }
    }

    fun load() {
        viewModelScope.launch {
            Library.cachedPlaylist(id)?.let { p -> _state.update { it.copy(playlist = p) } }
            try {
                val p = Library.fetchPlaylist(id)
                _state.update { it.copy(playlist = p, error = null) }
            } catch (e: SubsonicError) {
                _state.update { it.copy(error = errorMessage(e)) }
            }
        }
    }
}

/** A playlist (smart ones look the same): the album page's layout with each song's cover. */
@Composable
fun PlaylistScreen(id: String, onBack: () -> Unit) {
    val vm: PlaylistViewModel = viewModel(key = "playlist-$id") { PlaylistViewModel(id) }
    val state by vm.state.collectAsStateWithLifecycle()
    val playlist = state.playlist
    if (playlist == null) {
        Loading(state.error, vm::load, onBack)
        return
    }
    SongsPage(
        label = "PLAYLIST",
        title = playlist.name,
        note = playlist.comment?.takeIf { it.isNotBlank() },
        coverArt = playlist.coverArt,
        songs = playlist.entry,
        seconds = playlist.duration.takeIf { it > 0 } ?: playlist.entry.sumOf { it.duration ?: 0 },
        onBack = onBack,
        coverModifier = Modifier.itemGestures(Item.OfPlaylist(playlist), onClick = {}, role = null),
        download = Item.OfPlaylist(playlist),
    )
}

// ---- Favourite songs -------------------------------------------------------------------------

@Immutable
data class StarredSongsState(val songs: List<Song>? = null, val error: String? = null)

class StarredSongsViewModel : ViewModel() {
    private val _state = MutableStateFlow(StarredSongsState())
    val state: StateFlow<StarredSongsState> = _state.asStateFlow()

    init {
        load()
    }

    fun load() {
        viewModelScope.launch {
            Library.cachedStarred()?.let { s -> _state.update { it.copy(songs = s.song) } }
            try {
                val s = Library.fetchStarred()
                _state.update { it.copy(songs = s.song, error = null) }
            } catch (e: SubsonicError) {
                _state.update { it.copy(error = errorMessage(e)) }
            }
        }
    }
}

/** The favourite songs (the Library's Favourites chip), as a playlist-like page. */
@Composable
fun FavouriteSongsScreen(onBack: () -> Unit) {
    val vm: StarredSongsViewModel = viewModel { StarredSongsViewModel() }
    val state by vm.state.collectAsStateWithLifecycle()
    val favourites by Library.favourites.collectAsStateWithLifecycle()
    val all = state.songs
    if (all == null) {
        Loading(state.error, vm::load, onBack)
        return
    }
    // Unfavourited here (in a read-write build) leaves the list at once.
    val songs = remember(all, favourites) { all.filter { Library.isFavourite(it.id, it.starred, favourites) } }
    SongsPage(
        label = "FAVOURITES",
        title = "Favourite songs",
        note = null,
        coverArt = songs.firstOrNull()?.coverArt,
        songs = songs,
        seconds = songs.sumOf { it.duration ?: 0 },
        onBack = onBack,
        coverModifier = Modifier,
        download = null,
    )
}

@Composable
private fun Loading(error: String?, retry: () -> Unit, onBack: () -> Unit) {
    Box(Modifier.fillMaxSize()) {
        LoadingOrError(error, retry)
        BackButton(onBack, Modifier.padding(top = LocalScreenTop.current + 4.dp, start = 8.dp))
    }
}

@Composable
private fun SongsPage(
    label: String,
    title: String,
    note: String?,
    coverArt: String?,
    songs: List<Song>,
    seconds: Int,
    onBack: () -> Unit,
    coverModifier: Modifier,
    download: Item.OfPlaylist?,
) {
    val player = LocalPlayer.current
    val palette = Palette.current
    CollectionPage(
        coverArt = coverArt,
        coverDescription = "Cover of $title",
        onBack = onBack,
        coverModifier = coverModifier,
        info = { wide ->
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(label, style = MaterialTheme.typography.labelSmall, color = palette.textDim)
                Text(
                    title,
                    style = if (wide) MaterialTheme.typography.headlineSmall else MaterialTheme.typography.displaySmall,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.semantics { heading() },
                )
                if (note != null) {
                    Text(note, style = MaterialTheme.typography.bodyMedium, color = palette.textDim, maxLines = 3, overflow = TextOverflow.Ellipsis)
                }
                Text(songsLabel(songs.size, seconds), style = MaterialTheme.typography.bodyMedium, color = palette.textDim)
            }
        },
        buttons = { m ->
            // Wraps rather than pushing Download off a narrow phone with a large font.
            @OptIn(ExperimentalLayoutApi::class)
            FlowRow(m, horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                AccentButton("Play", Icons.Rounded.PlayArrow, onClick = { player.playSongs(songs, 0) }, enabled = songs.isNotEmpty())
                QuietButton("Shuffle", Icons.Rounded.Shuffle, onClick = { player.playSongs(songs, 0, shuffled = true) }, enabled = songs.isNotEmpty())
                if (download != null) DownloadButton(download, PinKind.Playlist, download.playlist.id, download.playlist.name)
            }
        },
        rows = {
            if (songs.isEmpty()) {
                item(key = "empty") { Text("No songs.", color = palette.textDim, modifier = Modifier.padding(16.dp)) }
            }
            itemsIndexed(songs, key = { i, s -> "s$i-${s.id}" }, contentType = { _, _ -> "song" }) { i, s ->
                SongRow(s, SongLead.Cover, onClick = { player.playSongs(songs, i) }, showAlbum = true)
            }
        },
    )
}

// ---- Genre -------------------------------------------------------------------------------

/** A genre's albums (`getAlbumList2 byGenre`), a page at a time. */
class GenreViewModel(private val genre: String) : ViewModel() {
    private val _state = MutableStateFlow(GridState())
    val state: StateFlow<GridState> = _state.asStateFlow()
    private var job: Job? = null
    private val _shuffling = MutableStateFlow(false)
    val shuffling: StateFlow<Boolean> = _shuffling.asStateFlow()

    init {
        loadMore()
    }

    fun loadMore() {
        val s = _state.value
        if (s.end || job?.isActive == true) return
        job = viewModelScope.launch {
            _state.update { it.copy(loading = true, error = null) }
            try {
                val page = Library.genreAlbums(genre, s.albums.size)
                _state.update { cur ->
                    val seen = cur.albums.mapTo(HashSet()) { it.id }
                    cur.copy(albums = cur.albums + page.filter { seen.add(it.id) }, loading = false, end = page.size < AlbumPaging.PAGE_SIZE)
                }
            } catch (e: SubsonicError) {
                _state.update { it.copy(loading = false, error = errorMessage(e)) }
            }
        }
    }

    /** Up to 300 of the genre's songs (`getSongsByGenre`), shuffled. */
    fun shuffle(play: (List<Song>) -> Unit) {
        if (_shuffling.value) return
        _shuffling.value = true
        viewModelScope.launch {
            try {
                val songs = Library.genreSongs(genre)
                if (songs.isEmpty()) Library.post("No songs in $genre") else play(songs)
            } catch (e: SubsonicError) {
                Library.post(errorMessage(e))
            } finally {
                _shuffling.value = false
            }
        }
    }
}

@Composable
fun GenreScreen(genre: String, albumCount: Int, songCount: Int, onBack: () -> Unit, onOpenAlbum: (String) -> Unit) {
    val vm: GenreViewModel = viewModel(key = "genre-$genre") { GenreViewModel(genre) }
    val state by vm.state.collectAsStateWithLifecycle()
    val shuffling by vm.shuffling.collectAsStateWithLifecycle()
    val player = LocalPlayer.current
    val palette = Palette.current
    PagedAlbumGrid(state, vm::loadMore, onOpenAlbum) {
        Column(Modifier.fillMaxWidth().padding(top = 4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                BackButton(onBack)
                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                    Text("GENRE", style = MaterialTheme.typography.labelSmall, color = palette.textDim)
                    Text(
                        genre,
                        style = MaterialTheme.typography.headlineSmall,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.semantics { heading() },
                    )
                }
            }
            Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                AccentButton("Shuffle genre", Icons.Rounded.Shuffle, onClick = { vm.shuffle { player.playSongs(it, 0) } }, enabled = !shuffling && songCount > 0)
                if (shuffling) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                Text(
                    listOf(albumsLabel(albumCount), songCountLabel(songCount)).joinToString(" · "),
                    style = MaterialTheme.typography.bodyMedium,
                    color = palette.textDim,
                )
            }
        }
    }
}
