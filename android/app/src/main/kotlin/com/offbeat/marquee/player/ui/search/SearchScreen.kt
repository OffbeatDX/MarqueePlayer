package com.offbeat.marquee.player.ui.search

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyGridScope
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.History
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import com.offbeat.marquee.core.subsonic.SearchResult
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.session.errorMessage
import com.offbeat.marquee.player.settings.LibraryPrefs
import com.offbeat.marquee.player.ui.LocalPlayer
import com.offbeat.marquee.player.ui.components.AlbumTile
import com.offbeat.marquee.player.ui.components.ArtistRow
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.components.LocalWide
import com.offbeat.marquee.player.ui.components.SectionHeader
import com.offbeat.marquee.player.ui.components.SongLead
import com.offbeat.marquee.player.ui.components.SongRow
import com.offbeat.marquee.player.ui.home.AlbumCellMin
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import androidx.compose.ui.graphics.Color

/** What a search shows: one more of each than is drawn is asked for, so "See all" knows there are more. */
private const val ARTISTS_SHOWN = 5
private const val ALBUMS_SHOWN = 12
private const val SONGS_SHOWN = 10

@Immutable
data class SearchState(
    val query: String = "",
    val result: SearchResult? = null,
    /** The query [result] answers (the field may have moved on). */
    val answered: String = "",
    val searching: Boolean = false,
    val error: String? = null,
)

/**
 * `search3` as the user types: 300 ms after the last keystroke, and a newer query cancels the one
 * in flight (its request is dropped).
 */
@OptIn(FlowPreview::class)
class SearchViewModel : ViewModel() {
    private val _state = MutableStateFlow(SearchState())
    val state: StateFlow<SearchState> = _state.asStateFlow()
    private val query = MutableStateFlow("")

    init {
        viewModelScope.launch {
            query.map { it.trim() }.distinctUntilChanged().debounce(300).collectLatest { q ->
                if (q.isEmpty()) {
                    _state.value = _state.value.copy(result = null, answered = "", searching = false, error = null)
                    return@collectLatest
                }
                _state.value = _state.value.copy(searching = true, error = null)
                try {
                    val r = Library.search(q, ARTISTS_SHOWN + 1, ALBUMS_SHOWN + 1, SONGS_SHOWN + 1)
                    _state.value = _state.value.copy(result = r, answered = q, searching = false)
                } catch (e: SubsonicError) {
                    // The previous query's answer goes: under the new query it would pass for its results.
                    _state.value = _state.value.copy(result = null, answered = "", searching = false, error = errorMessage(e))
                }
            }
        }
    }

    fun onQuery(q: String) {
        _state.value = _state.value.copy(query = q)
        query.value = q
    }
}

/**
 * Search: a field at the top (focused, keyboard up, whenever the tab is tapped: [focusRequest]
 * counts the taps), recent searches while it is empty, then Artists, Albums and Songs. Tapping a
 * song plays it with the rest of the song results queued after it.
 */
@Composable
fun SearchScreen(
    focusRequest: Int,
    onOpenAlbum: (String) -> Unit,
    onOpenArtist: (String) -> Unit,
    onSeeAll: (query: String, kind: String) -> Unit,
    vm: SearchViewModel = viewModel { SearchViewModel() },
) {
    val state by vm.state.collectAsStateWithLifecycle()
    val recent by LibraryPrefs.recent.collectAsStateWithLifecycle()
    val focus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    val focusManager = LocalFocusManager.current
    val player = LocalPlayer.current
    // Each tab tap focuses once; coming back from a result does not pop the keyboard again.
    var handled by rememberSaveable { mutableIntStateOf(0) }
    LaunchedEffect(focusRequest) {
        if (focusRequest != handled) {
            handled = focusRequest
            focus.requestFocus()
            keyboard?.show()
        }
    }
    val keep = { LibraryPrefs.addRecent(state.query) }
    val openAlbum = { id: String -> keep(); onOpenAlbum(id) }
    val openArtist = { id: String -> keep(); onOpenArtist(id) }
    val playSongs = { songs: List<Song>, i: Int -> keep(); focusManager.clearFocus(); player.playSongs(songs, i) }

    // imePadding: with the keyboard up, the results end above it (the scaffold has already taken off
    // what the bottom bar and mini player cover).
    Column(Modifier.fillMaxSize().padding(top = LocalScreenTop.current).imePadding()) {
        TextField(
            value = state.query,
            onValueChange = vm::onQuery,
            placeholder = { Text("Artists, albums, songs") },
            leadingIcon = { Icon(Icons.Rounded.Search, contentDescription = null) },
            trailingIcon = {
                if (state.searching) {
                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                } else if (state.query.isNotEmpty()) {
                    IconButton(onClick = { vm.onQuery(""); focus.requestFocus() }) { Icon(Icons.Rounded.Close, contentDescription = "Clear") }
                }
            },
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = { keep(); keyboard?.hide() }),
            shape = RoundedCornerShape(28.dp),
            colors = TextFieldDefaults.colors(
                focusedContainerColor = Palette.current.surfaceHi,
                unfocusedContainerColor = Palette.current.surface,
                focusedIndicatorColor = Color.Transparent,
                unfocusedIndicatorColor = Color.Transparent,
            ),
            modifier = Modifier.fillMaxWidth().padding(horizontal = Tokens.PageMargin, vertical = 8.dp).focusRequester(focus),
        )
        val result = state.result
        when {
            state.query.isBlank() -> Recent(recent, onPick = { vm.onQuery(it); LibraryPrefs.addRecent(it) }, onRemove = LibraryPrefs::removeRecent)
            state.error != null && result == null -> Note(state.error.orEmpty())
            result == null -> Unit
            result.artist.isEmpty() && result.album.isEmpty() && result.song.isEmpty() -> Note("Nothing found for “${state.answered}”.")
            LocalWide.current -> WideResults(result, state.answered, openAlbum, openArtist, playSongs, onSeeAll)
            else -> NarrowResults(result, state.answered, openAlbum, openArtist, playSongs, onSeeAll)
        }
    }
}

@Composable
private fun Note(text: String) {
    Text(text, color = Palette.current.textDim, modifier = Modifier.padding(horizontal = Tokens.PageMargin, vertical = 16.dp))
}

/** The last ten searches, newest first; each can be removed. Kept on this device only. */
@Composable
private fun Recent(recent: List<String>, onPick: (String) -> Unit, onRemove: (String) -> Unit) {
    val palette = Palette.current
    if (recent.isEmpty()) return Note("Search your library.")
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item(key = "title") {
            Text(
                "Recent searches",
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(horizontal = Tokens.PageMargin, vertical = 8.dp).semantics { heading() },
            )
        }
        items(recent, key = { it }) { q ->
            Row(
                Modifier.fillMaxWidth().heightIn(min = 52.dp).clickable(role = Role.Button, onClickLabel = "Search again") { onPick(q) }
                    .padding(start = Tokens.PageMargin, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.Rounded.History, contentDescription = null, tint = palette.textDim)
                Text(q, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f).padding(horizontal = 16.dp))
                IconButton(onClick = { onRemove(q) }, modifier = Modifier.size(48.dp)) {
                    Icon(Icons.Rounded.Close, contentDescription = "Remove $q from recent searches", tint = palette.textDim)
                }
            }
        }
    }
}

/** Portrait: artists, a row of albums, songs. */
@Composable
private fun NarrowResults(
    r: SearchResult,
    query: String,
    openAlbum: (String) -> Unit,
    openArtist: (String) -> Unit,
    play: (List<Song>, Int) -> Unit,
    onSeeAll: (String, String) -> Unit,
) {
    val songs = r.song.take(SONGS_SHOWN)
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        section(r.artist.size, ARTISTS_SHOWN, "Artists") { onSeeAll(query, KIND_ARTISTS) }
        items(r.artist.take(ARTISTS_SHOWN), key = { "ar-${it.id}" }, contentType = { "artist" }) { ArtistRow(it, openArtist) }
        section(r.album.size, ALBUMS_SHOWN, "Albums") { onSeeAll(query, KIND_ALBUMS) }
        if (r.album.isNotEmpty()) {
            item(key = "albums", contentType = "albums") {
                LazyRow(contentPadding = PaddingValues(horizontal = Tokens.PageMargin), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    items(r.album.take(ALBUMS_SHOWN), key = { it.id }) { AlbumTile(it, openAlbum, Modifier.width(140.dp)) }
                }
            }
        }
        section(r.song.size, SONGS_SHOWN, "Songs") { onSeeAll(query, KIND_SONGS) }
        itemsIndexed(songs, key = { i, s -> "s$i-${s.id}" }, contentType = { _, _ -> "song" }) { i, s ->
            SongRow(s, SongLead.Cover, onClick = { play(songs, i) }, showAlbum = true)
        }
    }
}

private fun LazyListScope.section(count: Int, shown: Int, title: String, onSeeAll: () -> Unit) {
    if (count == 0) return
    item(key = "t-$title", contentType = "title") {
        SectionHeader(title, Modifier.padding(start = Tokens.PageMargin, end = 8.dp, top = 8.dp), onOpen = if (count > shown) onSeeAll else null)
    }
}

/** Wide: artists and songs in two columns over the width, albums in a row between them. */
@Composable
private fun WideResults(
    r: SearchResult,
    query: String,
    openAlbum: (String) -> Unit,
    openArtist: (String) -> Unit,
    play: (List<Song>, Int) -> Unit,
    onSeeAll: (String, String) -> Unit,
) {
    val songs = r.song.take(SONGS_SHOWN)
    LazyVerticalGrid(
        columns = GridCells.Adaptive(AlbumCellMin),
        contentPadding = PaddingValues(bottom = 24.dp),
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        modifier = Modifier.fillMaxSize(),
    ) {
        gridSection(r.artist.size, ARTISTS_SHOWN, "Artists") { onSeeAll(query, KIND_ARTISTS) }
        items(r.artist.take(ARTISTS_SHOWN), key = { "ar-${it.id}" }, span = { GridItemSpan(half(maxLineSpan)) }, contentType = { "artist" }) { ArtistRow(it, openArtist) }
        gridSection(r.album.size, ALBUMS_SHOWN, "Albums") { onSeeAll(query, KIND_ALBUMS) }
        if (r.album.isNotEmpty()) {
            item(key = "albums", span = { GridItemSpan(maxLineSpan) }, contentType = "albums") {
                LazyRow(contentPadding = PaddingValues(horizontal = Tokens.PageMargin), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    items(r.album.take(ALBUMS_SHOWN), key = { it.id }) { AlbumTile(it, openAlbum, Modifier.width(150.dp)) }
                }
            }
        }
        gridSection(r.song.size, SONGS_SHOWN, "Songs") { onSeeAll(query, KIND_SONGS) }
        itemsIndexed(songs, key = { i, s -> "s$i-${s.id}" }, span = { _, _ -> GridItemSpan(half(maxLineSpan)) }, contentType = { _, _ -> "song" }) { i, s ->
            SongRow(s, SongLead.Cover, onClick = { play(songs, i) }, showAlbum = true)
        }
    }
}

private fun half(line: Int) = if (line >= 4) line / 2 else line

private fun LazyGridScope.gridSection(count: Int, shown: Int, title: String, onSeeAll: () -> Unit) {
    if (count == 0) return
    item(key = "t-$title", span = { GridItemSpan(maxLineSpan) }, contentType = "title") {
        SectionHeader(title, Modifier.padding(start = Tokens.PageMargin, end = 8.dp, top = 8.dp), onOpen = if (count > shown) onSeeAll else null)
    }
}

const val KIND_ARTISTS = "artists"
const val KIND_ALBUMS = "albums"
const val KIND_SONGS = "songs"
