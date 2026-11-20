package com.offbeat.marquee.player.ui.artist

import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyGridScope
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Shuffle
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.art.Backdrop
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.data.ArtistPage
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.session.errorMessage
import com.offbeat.marquee.player.ui.LocalPlayer
import com.offbeat.marquee.player.ui.components.AccentButton
import com.offbeat.marquee.player.ui.components.AlbumTile
import com.offbeat.marquee.player.ui.components.BackButton
import com.offbeat.marquee.player.ui.components.LoadingOrError
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.components.LocalWide
import com.offbeat.marquee.player.ui.components.QuietButton
import com.offbeat.marquee.player.ui.components.SectionHeader
import com.offbeat.marquee.player.ui.components.SongLead
import com.offbeat.marquee.player.ui.components.SongRow
import com.offbeat.marquee.player.ui.components.formatTime
import com.offbeat.marquee.player.ui.playingSongId
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.ui.components.rememberKeptGridState
import com.offbeat.marquee.core.subsonic.Album
import androidx.compose.ui.unit.Dp

@Immutable
data class ArtistState(val page: ArtistPage? = null, val error: String? = null, val shuffling: Boolean = false)

class ArtistViewModel(private val id: String) : ViewModel() {
    private val _state = MutableStateFlow(ArtistState())
    val state: StateFlow<ArtistState> = _state.asStateFlow()

    init {
        load()
        viewModelScope.launch { NetworkMonitor.cameBack.collect { load() } }
    }

    fun load() {
        viewModelScope.launch {
            Library.cachedArtist(id)?.let { p -> _state.update { it.copy(page = p) } }
            try {
                val p = Library.fetchArtist(id)
                _state.update { it.copy(page = p, error = null) }
            } catch (e: SubsonicError) {
                _state.update { it.copy(error = errorMessage(e)) }
            }
        }
    }

    /** Every song of the artist's albums, shuffled (capped), handed to [play]. */
    fun shuffleAll(play: (List<Song>) -> Unit) {
        val page = _state.value.page ?: return
        if (_state.value.shuffling) return
        _state.update { it.copy(shuffling = true) }
        viewModelScope.launch {
            val songs = Library.shuffledSongs(page)
            _state.update { it.copy(shuffling = false) }
            if (songs.isEmpty()) Library.post("Couldn't load the artist's songs") else play(songs)
        }
    }
}

@Composable
fun ArtistScreen(id: String, onBack: () -> Unit, onOpenAlbum: (String) -> Unit) {
    val vm: ArtistViewModel = viewModel(key = "artist-$id") { ArtistViewModel(id) }
    val state by vm.state.collectAsStateWithLifecycle()
    val page = state.page
    val player = LocalPlayer.current
    val playing by playingSongId()
    val top = LocalScreenTop.current
    // One state per layout, each kept across rotations (turning the phone and back returns to the same place).
    val grid = rememberKeptGridState()
    val leftPane = rememberKeptGridState()
    Box(Modifier.fillMaxSize()) {
        if (page == null) {
            LoadingOrError(state.error, vm::load)
        } else {
            val albums = page.artist.album.sortedWith(compareByDescending<Album> { it.year ?: 0 }.thenBy { it.name })
            val playTop = { i: Int -> player.playSongs(page.top, i) }
            val shuffleAll = { vm.shuffleAll { player.playSongs(it, 0) } }
            if (LocalWide.current) {
                Row(Modifier.fillMaxSize()) {
                    LazyVerticalGrid(
                        columns = GridCells.Fixed(1),
                        state = leftPane,
                        modifier = Modifier.weight(0.42f).fillMaxHeight(),
                        contentPadding = PaddingValues(bottom = 24.dp),
                    ) {
                        item(key = "header") { Header(page, albums.firstOrNull()?.coverArt, top, wide = true) }
                        item(key = "buttons") { Buttons(page, state.shuffling, playTop, shuffleAll) }
                        topSongs(page.top, playing, playTop, bleed = false)
                    }
                    LazyVerticalGrid(
                        columns = GridCells.Adaptive(140.dp),
                        modifier = Modifier.weight(0.58f).fillMaxHeight(),
                        contentPadding = PaddingValues(start = Tokens.PageMargin, end = Tokens.PageMargin, top = top + 8.dp, bottom = 24.dp),
                        horizontalArrangement = Arrangement.spacedBy(14.dp),
                        verticalArrangement = Arrangement.spacedBy(14.dp),
                    ) { albumGrid(albums, onOpenAlbum) }
                }
            } else {
                // The grid keeps the page margin; the header and lists bleed past it to the edges.
                LazyVerticalGrid(
                    columns = GridCells.Adaptive(140.dp),
                    state = grid,
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(start = Tokens.PageMargin, end = Tokens.PageMargin, bottom = 24.dp),
                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                    verticalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    item(key = "header", span = { GridItemSpan(maxLineSpan) }) {
                        Box(Modifier.fullBleed(Tokens.PageMargin)) { Header(page, albums.firstOrNull()?.coverArt, top, wide = false) }
                    }
                    item(key = "buttons", span = { GridItemSpan(maxLineSpan) }) {
                        Box(Modifier.fullBleed(Tokens.PageMargin)) { Buttons(page, state.shuffling, playTop, shuffleAll) }
                    }
                    topSongs(page.top, playing, playTop, bleed = true)
                    albumGrid(albums, onOpenAlbum)
                }
            }
        }
        BackButton(onBack, Modifier.padding(top = top + 4.dp, start = 8.dp))
    }
}

/**
 * The artist's picture when the server has one (through `getCoverArt`), otherwise the blurred art
 * of their newest album; the name over its lower edge.
 */
@Composable
private fun Header(page: ArtistPage, fallbackCover: String?, top: Dp, wide: Boolean) {
    val palette = Palette.current
    Box(
        Modifier
            .fillMaxWidth()
            // Wide windows are often short: half their height, so the buttons stay in view.
            .then(if (wide) Modifier.height(LocalConfiguration.current.screenHeightDp.dp * 0.5f + top) else Modifier.height(top + 260.dp)),
    ) {
        if (page.imageCoverId != null) {
            CoverImage(page.imageCoverId, Modifier.matchParentSize(), shape = RectangleShape, contentDescription = null)
        } else {
            Backdrop(fallbackCover, Color(0x60000000), Modifier.matchParentSize())
        }
        Box(Modifier.matchParentSize().background(Brush.verticalGradient(0.45f to Color.Transparent, 1f to palette.bg)))
        Column(Modifier.align(Alignment.BottomStart).padding(horizontal = Tokens.PageMargin, vertical = 8.dp)) {
            Text("ARTIST", style = MaterialTheme.typography.labelSmall, color = palette.textDim)
            Text(
                page.artist.name,
                style = MaterialTheme.typography.displaySmall,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.semantics { heading() },
            )
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Buttons(page: ArtistPage, shuffling: Boolean, playTop: (Int) -> Unit, shuffleAll: () -> Unit) {
    // A FlowRow: with a large font on a narrow phone the last buttons wrap to a second line
    // instead of falling off the edge (font size 2.0 at 360 dp could hide Shuffle all).
    FlowRow(
        Modifier.padding(horizontal = Tokens.PageMargin, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        AccentButton("Top songs", Icons.Rounded.PlayArrow, onClick = { playTop(0) }, enabled = page.top.isNotEmpty())
        QuietButton("Shuffle all", Icons.Rounded.Shuffle, onClick = shuffleAll, enabled = !shuffling && page.artist.album.isNotEmpty())
        if (shuffling) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
    }
}

private fun LazyGridScope.topSongs(top: List<Song>, playing: String, play: (Int) -> Unit, bleed: Boolean) {
    if (top.isEmpty()) return
    val edge = if (bleed) Modifier.fullBleed(Tokens.PageMargin) else Modifier
    item(key = "top-title", span = { GridItemSpan(maxLineSpan) }) {
        SectionHeader("Top songs", edge.padding(horizontal = Tokens.PageMargin))
    }
    itemsIndexed(top.take(5), key = { i, s -> "top$i-${s.id}" }, span = { _, _ -> GridItemSpan(maxLineSpan) }, contentType = { _, _ -> "song" }) { i, s ->
        SongRow(s, SongLead.Cover, onClick = { play(i) }, modifier = edge, showAlbum = true)
    }
}

private fun LazyGridScope.albumGrid(albums: List<Album>, onOpen: (String) -> Unit) {
    if (albums.isEmpty()) return
    item(key = "albums-title", span = { GridItemSpan(maxLineSpan) }) { SectionHeader("Albums") }
    items(albums, key = { it.id }, contentType = { "album" }) { a -> AlbumTile(a, onOpen) }
}

/** Lets a full-span item in a padded grid reach the screen edges. */
private fun Modifier.fullBleed(margin: Dp): Modifier = layout { measurable, constraints ->
    val extra = margin.roundToPx() * 2
    val width = constraints.maxWidth + extra
    val placeable = measurable.measure(constraints.copy(minWidth = width, maxWidth = width))
    layout(constraints.maxWidth, placeable.height) { placeable.place(-extra / 2, 0) }
}
