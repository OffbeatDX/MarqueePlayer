package com.offbeat.marquee.player.ui.album

import com.offbeat.marquee.player.AppScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Favorite
import androidx.compose.material.icons.rounded.FavoriteBorder
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Shuffle
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
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import com.offbeat.marquee.core.library.ActionPlace
import com.offbeat.marquee.core.subsonic.Album
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
import com.offbeat.marquee.player.ui.components.AccentButton
import com.offbeat.marquee.player.ui.components.BackButton
import com.offbeat.marquee.player.ui.components.CollectionPage
import com.offbeat.marquee.player.ui.components.LoadingOrError
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.components.QuietButton
import com.offbeat.marquee.player.ui.components.SongLead
import com.offbeat.marquee.player.ui.components.SongRow
import com.offbeat.marquee.player.ui.components.formatLength
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

@Immutable
data class AlbumState(val album: Album? = null, val error: String? = null)

/** An album: from the disk cache at once when it was opened before, then from the server. */
class AlbumViewModel(private val id: String) : ViewModel() {
    private val _state = MutableStateFlow(AlbumState())
    val state: StateFlow<AlbumState> = _state.asStateFlow()

    init {
        load()
        // Back online: this page refreshes once.
        viewModelScope.launch { NetworkMonitor.cameBack.collect { load() } }
    }

    fun load() {
        viewModelScope.launch {
            Library.cachedAlbum(id)?.let { a -> _state.update { it.copy(album = a) } }
            try {
                val a = Library.fetchAlbum(id)
                _state.update { it.copy(album = a, error = null) }
            } catch (e: SubsonicError) {
                _state.update { it.copy(error = errorMessage(e)) }
            }
        }
    }
}

/** A disc's header, or a track, as one lazy list item. */
private sealed interface ListRow {
    data class Disc(val number: Int) : ListRow
    data class Track(val index: Int, val song: Song) : ListRow
}

@Composable
fun AlbumScreen(id: String, onBack: () -> Unit, onOpenArtist: (String) -> Unit) {
    val vm: AlbumViewModel = viewModel(key = "album-$id") { AlbumViewModel(id) }
    val state by vm.state.collectAsStateWithLifecycle()
    val album = state.album
    val player = LocalPlayer.current
    val favourites by Library.favourites.collectAsStateWithLifecycle()
    // Not the screen's scope: leaving the page must not stop a pin, a removal or a heart half way.
    val scope = AppScope
    if (album == null) {
        Box(Modifier.fillMaxSize()) {
            LoadingOrError(state.error, vm::load)
            BackButton(onBack, Modifier.padding(top = LocalScreenTop.current + 4.dp, start = 8.dp))
        }
        return
    }
    val songs = album.song
    val rows = remember(songs) { rowsOf(songs) }
    val favourite = Library.isFavourite(album.id, album.starred, favourites)
    val play = { index: Int, shuffled: Boolean -> player.playSongs(songs, index, shuffled) }
    val toggleFavourite = { scope.launch { Library.setFavourite(Item.OfAlbum(album), !favourite) }; Unit }
    CollectionPage(
        coverArt = album.coverArt,
        coverDescription = "Cover of ${album.name}",
        onBack = onBack,
        coverModifier = Modifier.itemGestures(Item.OfAlbum(album), onClick = {}, place = ActionPlace(onOwnPage = true), role = null),
        info = { wide -> Info(album, wide, onOpenArtist) },
        buttons = { m -> AlbumButtons(album, favourite, play, toggleFavourite, m) },
        rows = { tracks(rows, play) },
    )
}

private fun rowsOf(songs: List<Song>): List<ListRow> {
    val discs = songs.map { it.discNumber ?: 1 }.distinct()
    if (discs.size <= 1) return songs.mapIndexed { i, s -> ListRow.Track(i, s) }
    val out = ArrayList<ListRow>(songs.size + discs.size)
    var disc: Int? = null
    songs.forEachIndexed { i, s ->
        val d = s.discNumber ?: 1
        if (d != disc) {
            out += ListRow.Disc(d)
            disc = d
        }
        out += ListRow.Track(i, s)
    }
    return out
}

private fun LazyListScope.tracks(rows: List<ListRow>, play: (Int, Boolean) -> Unit) {
    items(
        rows,
        key = { r -> if (r is ListRow.Track) "t${r.index}-${r.song.id}" else "d${(r as ListRow.Disc).number}" },
        contentType = { if (it is ListRow.Track) "track" else "disc" },
    ) { r ->
        when (r) {
            is ListRow.Disc -> Text(
                "Disc ${r.number}",
                style = MaterialTheme.typography.labelSmall,
                color = Palette.current.textDim,
                modifier = Modifier.padding(start = Tokens.PageMargin, top = 18.dp, bottom = 6.dp).semantics { heading() },
            )
            is ListRow.Track -> SongRow(r.song, SongLead.Number(r.song.track), onClick = { play(r.index, false) })
        }
    }
}

/** "ALBUM", the title, the artist (opens it), then year, track count and length. */
@Composable
private fun Info(album: Album, wide: Boolean, onOpenArtist: (String) -> Unit) {
    val palette = Palette.current
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("ALBUM", style = MaterialTheme.typography.labelSmall, color = palette.textDim)
        Text(
            album.name,
            style = if (wide) MaterialTheme.typography.headlineSmall else MaterialTheme.typography.displaySmall,
            maxLines = if (wide) 3 else 4,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.semantics { heading() },
        )
        album.artist?.let { artist ->
            Text(
                artist,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier
                    .clickable(enabled = album.artistId != null, role = Role.Button, onClickLabel = "Open artist") {
                        album.artistId?.let(onOpenArtist)
                    }
                    .padding(vertical = 4.dp),
            )
        }
        val length = album.duration.takeIf { it > 0 } ?: album.song.sumOf { it.duration ?: 0 }
        val tracks = album.songCount.takeIf { it > 0 } ?: album.song.size
        Text(
            listOfNotNull(
                album.year?.takeIf { it > 0 }?.toString(),
                if (tracks == 1) "1 track" else "$tracks tracks",
                formatLength(length),
            ).joinToString(" · "),
            style = MaterialTheme.typography.bodyMedium,
            color = palette.textDim,
        )
    }
}

/** Play, Shuffle, the heart and Download (with its progress ring). */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun AlbumButtons(album: Album, favourite: Boolean, play: (Int, Boolean) -> Unit, onFavourite: () -> Unit, modifier: Modifier = Modifier) {
    val palette = Palette.current
    // A FlowRow: with a large font on a narrow phone the last buttons wrap to a second line
    // instead of falling off the edge (font size 2.0 at 360 dp hid Download).
    FlowRow(
        modifier,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        AccentButton("Play", Icons.Rounded.PlayArrow, onClick = { play(0, false) }, enabled = album.song.isNotEmpty())
        QuietButton("Shuffle", Icons.Rounded.Shuffle, onClick = { play(0, true) }, enabled = album.song.isNotEmpty())
        IconButton(onClick = onFavourite, modifier = Modifier.size(48.dp)) {
            Icon(
                if (favourite) Icons.Rounded.Favorite else Icons.Rounded.FavoriteBorder,
                contentDescription = if (favourite) "Remove the album from favourites" else "Add the album to favourites",
                tint = if (favourite) palette.accent else palette.textDim,
            )
        }
        DownloadButton(Item.OfAlbum(album), PinKind.Album, album.id, album.name)
    }
}
