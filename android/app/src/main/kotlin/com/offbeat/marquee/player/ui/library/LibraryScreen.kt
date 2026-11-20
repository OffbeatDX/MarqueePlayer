package com.offbeat.marquee.player.ui.library

import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.Sort
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.DownloadForOffline
import androidx.compose.material.icons.rounded.Favorite
import androidx.compose.material.icons.rounded.FavoriteBorder
import androidx.compose.material.icons.rounded.KeyboardArrowRight
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Tab
import androidx.compose.material3.ScrollableTabRow
import androidx.compose.material3.TabPosition
import androidx.compose.material3.TabRow
import androidx.compose.material3.TabRowDefaults
import androidx.compose.material3.TabRowDefaults.tabIndicatorOffset
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.unit.Constraints
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.offbeat.marquee.core.library.AlbumSort
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.Artist
import com.offbeat.marquee.core.subsonic.Genre
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.core.offline.PinKind
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.settings.LibraryPrefs
import com.offbeat.marquee.player.ui.components.AlbumPlaceholderTile
import com.offbeat.marquee.player.ui.components.AlbumTile
import com.offbeat.marquee.player.ui.components.AlphabetScroller
import com.offbeat.marquee.player.ui.components.ArtistRow
import com.offbeat.marquee.player.ui.components.FastScroller
import com.offbeat.marquee.player.ui.components.LetterBubble
import com.offbeat.marquee.player.ui.components.LoadingOrError
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.components.LocalWide
import com.offbeat.marquee.player.ui.components.PlaylistRow
import com.offbeat.marquee.player.ui.home.AlbumCellMin
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import com.offbeat.marquee.player.ui.actions.albumsLabel
import com.offbeat.marquee.player.ui.actions.songCountLabel
import com.offbeat.marquee.player.ui.components.formatBytes

/**
 * The Library: Albums, Artists, Playlists and Genres (the tab and the album sort are remembered
 * between runs), with the Favourites chip on Albums and Artists and Downloaded (what is pinned for
 * offline, from the phone alone) on Albums and Playlists.
 */
@Composable
fun LibraryScreen(
    onOpenAlbum: (String) -> Unit,
    onOpenArtist: (String) -> Unit,
    onOpenPlaylist: (String) -> Unit,
    onOpenGenre: (Genre) -> Unit,
    onOpenFavouriteSongs: () -> Unit,
    onOpenDownloads: () -> Unit,
    vm: LibraryViewModel = viewModel { LibraryViewModel() },
) {
    val tabIndex by LibraryPrefs.tab.collectAsStateWithLifecycle()
    val tab = LibraryTab.entries.getOrElse(tabIndex) { LibraryTab.Albums }
    var favourites by rememberSaveable { mutableStateOf(false) }
    var downloaded by rememberSaveable { mutableStateOf(false) }
    val chipDownloaded = downloaded && (tab == LibraryTab.Albums || tab == LibraryTab.Playlists)
    val chipFavourites = favourites && !chipDownloaded && (tab == LibraryTab.Albums || tab == LibraryTab.Artists)
    LaunchedEffect(tab, chipFavourites, chipDownloaded) { if (!chipDownloaded) vm.ensure(tab, chipFavourites) }
    val albums by vm.albums.collectAsStateWithLifecycle()
    val wide = LocalWide.current

    Column(Modifier.fillMaxSize().padding(top = LocalScreenTop.current)) {
        val tabs = @Composable { modifier: Modifier -> Tabs(tab, modifier) { LibraryPrefs.setTab(it.ordinal) } }
        val chips = @Composable {
            // Scrolls sideways when the chips don't fit (font size 2.0 on a 360 dp phone clipped
            // "Downloaded") rather than squeezing them.
            Row(
                Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (tab == LibraryTab.Albums || tab == LibraryTab.Artists) {
                    FilterChip(
                        selected = favourites,
                        onClick = { favourites = !favourites; if (favourites) downloaded = false },
                        label = { Text("Favourites", maxLines = 1, softWrap = false) },
                        leadingIcon = {
                            Icon(if (favourites) Icons.Rounded.Favorite else Icons.Rounded.FavoriteBorder, contentDescription = null, modifier = Modifier.size(18.dp))
                        },
                        colors = chipColors(),
                    )
                }
                if (tab == LibraryTab.Albums || tab == LibraryTab.Playlists) {
                    // What is pinned for offline: from the phone, no network needed.
                    FilterChip(
                        selected = downloaded,
                        onClick = { downloaded = !downloaded; if (downloaded) favourites = false },
                        label = { Text("Downloaded", maxLines = 1, softWrap = false) },
                        leadingIcon = { Icon(Icons.Rounded.DownloadForOffline, contentDescription = null, modifier = Modifier.size(18.dp)) },
                        colors = chipColors(),
                    )
                }
            }
        }
        val sortButton = @Composable { compact: Boolean -> if (tab == LibraryTab.Albums) SortMenu(albums.sort, compact, vm::setSort) }
        if (wide) {
            // One row, as a short window needs its height for the grid: the rail already says
            // "Library", the tabs keep their places whatever the tab, the sort shows as its icon.
            WideHeader(
                tabs = { tabs(Modifier) },
                tools = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        chips()
                        sortButton(true)
                    }
                },
                modifier = Modifier.fillMaxWidth().padding(end = 12.dp),
            )
        } else {
            Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(start = Tokens.PageMargin, end = 4.dp, top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Title()
                Spacer(Modifier.weight(1f))
                sortButton(false)
            }
            tabs(Modifier.fillMaxWidth())
            Box(Modifier.padding(horizontal = Tokens.PageMargin, vertical = 2.dp)) { chips() }
        }
        Box(Modifier.weight(1f)) {
            when (tab) {
                LibraryTab.Albums -> if (chipDownloaded) DownloadedAlbums(vm, onOpenAlbum, onOpenDownloads) else if (chipFavourites) FavouriteAlbums(vm, onOpenAlbum, onOpenFavouriteSongs) else AlbumsGrid(vm, albums, onOpenAlbum)
                LibraryTab.Artists -> ArtistsList(vm, chipFavourites, onOpenArtist)
                LibraryTab.Playlists -> if (chipDownloaded) DownloadedPlaylists(onOpenPlaylist, onOpenDownloads) else PlaylistsList(vm, onOpenPlaylist)
                LibraryTab.Genres -> GenresList(vm, onOpenGenre)
            }
        }
    }
}

/**
 * The wide header: the tabs (400 dp, more with a larger font) with the chips and sort beside them
 * when they fit, as a short window needs its height for the grid; otherwise (a small phone in
 * landscape, a large font) the chips and sort go on a line under the tabs. A fixed 400 dp row
 * squeezed the chips to two lines and pushed the sort off the screen at 760 x 360 dp.
 */
@Composable
private fun WideHeader(tabs: @Composable () -> Unit, tools: @Composable () -> Unit, modifier: Modifier) {
    Layout(contents = listOf(tabs, tools), modifier = modifier) { (tabsM, toolsM), c ->
        val w = c.maxWidth
        val tool = toolsM.map { it.measure(Constraints(maxWidth = w)) }
        val toolW = tool.maxOfOrNull { it.width } ?: 0
        val toolH = tool.maxOfOrNull { it.height } ?: 0
        val tabsW = (400.dp.toPx() * fontScale.coerceAtLeast(1f)).toInt()
        val minH = 56.dp.roundToPx()
        if (w - toolW >= tabsW) {
            val tab = tabsM.map { it.measure(Constraints.fixedWidth(tabsW)) }
            val h = maxOf(minH, toolH, tab.maxOfOrNull { it.height } ?: 0)
            layout(w, h) {
                tab.forEach { it.place(0, (h - it.height) / 2) }
                tool.forEach { it.place(w - toolW, (h - it.height) / 2) }
            }
        } else {
            val tab = tabsM.map { it.measure(Constraints.fixedWidth(w)) }
            val tabH = maxOf(minH, tab.maxOfOrNull { it.height } ?: 0)
            layout(w, tabH + toolH) {
                tab.forEach { it.place(0, (tabH - it.height) / 2) }
                tool.forEach { it.place(w - toolW, tabH) }
            }
        }
    }
}

@Composable
private fun Title() {
    Text("Library", style = MaterialTheme.typography.headlineMedium, modifier = Modifier.semantics { heading() })
}

@Composable
private fun chipColors() = FilterChipDefaults.filterChipColors(
    selectedContainerColor = Palette.current.accent.copy(alpha = 0.18f),
    selectedLabelColor = Palette.current.accent,
    selectedLeadingIconColor = Palette.current.accent,
)

/**
 * The four tabs share the width equally while their labels fit; with a large font (2.0 clipped
 * them to "Albu", "Playli") they become a scrolling row instead, each as wide as its label.
 */
@Composable
private fun Tabs(tab: LibraryTab, modifier: Modifier, onTab: (LibraryTab) -> Unit) {
    val palette = Palette.current
    val style = MaterialTheme.typography.labelLarge
    val measurer = rememberTextMeasurer()
    val widest = remember(style, measurer) { LibraryTab.entries.maxOf { measurer.measure(it.title, style, maxLines = 1).size.width } }
    val indicator = @Composable { positions: List<TabPosition> ->
        TabRowDefaults.SecondaryIndicator(Modifier.tabIndicatorOffset(positions[tab.ordinal]), color = palette.accent)
    }
    val tabs = @Composable {
        for (t in LibraryTab.entries) {
            Tab(
                selected = t == tab,
                onClick = { onTab(t) },
                text = { Text(t.title, maxLines = 1, softWrap = false, style = style) },
                selectedContentColor = palette.accent,
                unselectedContentColor = palette.textDim,
            )
        }
    }
    BoxWithConstraints(modifier) {
        // A tab pads its label by 16 dp on each side.
        val fits = with(LocalDensity.current) { (widest.toDp() + 32.dp) * LibraryTab.entries.size <= maxWidth }
        if (fits) {
            TabRow(
                selectedTabIndex = tab.ordinal,
                containerColor = androidx.compose.ui.graphics.Color.Transparent,
                contentColor = palette.text,
                divider = {},
                indicator = indicator,
                tabs = tabs,
            )
        } else {
            ScrollableTabRow(
                selectedTabIndex = tab.ordinal,
                containerColor = androidx.compose.ui.graphics.Color.Transparent,
                contentColor = palette.text,
                edgePadding = 0.dp,
                divider = {},
                indicator = indicator,
                tabs = tabs,
            )
        }
    }
}

@Composable
private fun SortMenu(sort: AlbumSort, compact: Boolean, onSort: (AlbumSort) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val palette = Palette.current
    Box {
        if (compact) {
            androidx.compose.material3.IconButton(onClick = { open = true }, modifier = Modifier.size(48.dp)) {
                Icon(Icons.AutoMirrored.Rounded.Sort, contentDescription = "Sort albums: ${sort.title}", tint = palette.textDim)
            }
        } else {
            TextButton(onClick = { open = true }, modifier = Modifier.heightIn(min = 48.dp)) {
                Icon(Icons.AutoMirrored.Rounded.Sort, contentDescription = null, tint = palette.textDim, modifier = Modifier.size(20.dp))
                Spacer(Modifier.width(6.dp))
                Text(sort.title, color = palette.textDim, style = MaterialTheme.typography.labelLarge)
            }
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            Text("Sort albums by", style = MaterialTheme.typography.labelSmall, color = palette.textDim, modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp))
            for (s in AlbumSort.entries) {
                DropdownMenuItem(
                    text = { Text(s.title, color = if (s == sort) palette.accent else palette.text) },
                    trailingIcon = { if (s == sort) Icon(Icons.Rounded.Check, contentDescription = "Selected", tint = palette.accent) },
                    onClick = { open = false; onSort(s) },
                )
            }
        }
    }
}

// ---- Albums -----------------------------------------------------------------------------------

private val gridPadding = PaddingValues(start = Tokens.PageMargin, end = Tokens.PageMargin, top = 8.dp, bottom = 24.dp)

@Composable
private fun AlbumsGrid(vm: LibraryViewModel, state: AlbumsState, onOpenAlbum: (String) -> Unit) {
    // A new sort starts at the top; a rotation keeps the place.
    val grid = rememberSaveable(state.sort, saver = LazyGridState.Saver) { LazyGridState() }
    val articles by vm.articles.collectAsStateWithLifecycle()
    LaunchedEffect(state.sort) { if (state.sort.bubble) vm.ensureArticles() }
    LaunchedEffect(grid) {
        snapshotFlow {
            val items = grid.layoutInfo.visibleItemsInfo
            (items.firstOrNull()?.index ?: 0) to (items.lastOrNull()?.index ?: -1)
        }.distinctUntilChanged().collect { (first, last) -> vm.onVisible(first, last) }
    }
    if (state.count == 0) {
        if (state.loading || state.error != null) {
            LoadingOrError(state.error, vm::retryAlbums)
        } else {
            Empty("No albums yet.")
        }
        return
    }
    Box(Modifier.fillMaxSize()) {
        LazyVerticalGrid(
            columns = GridCells.Adaptive(AlbumCellMin),
            state = grid,
            contentPadding = gridPadding,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
            modifier = Modifier.fillMaxSize(),
        ) {
            // Positional cells: the list is in a fixed order, and cells fill in as pages arrive.
            items(state.count, contentType = { "album" }) { i ->
                val album = state.albumAt(i)
                if (album == null) AlbumPlaceholderTile() else AlbumTile(album, onOpenAlbum)
            }
        }
        if (state.sort.pagesByOffset) {
            FastScroller(
                grid,
                state.count,
                bubble = if (state.sort.bubble) { i -> state.nearestAlbum(i)?.let { state.sort.bubbleText(it, articles) } } else null,
                modifier = Modifier.align(Alignment.TopEnd).padding(top = 8.dp, bottom = 8.dp),
            )
        }
    }
}

@Composable
private fun FavouriteAlbums(vm: LibraryViewModel, onOpenAlbum: (String) -> Unit, onOpenSongs: () -> Unit) {
    val starred by vm.starred.collectAsStateWithLifecycle()
    val overrides by Library.favourites.collectAsStateWithLifecycle()
    val sort by LibraryPrefs.sort.collectAsStateWithLifecycle()
    val articles by vm.articles.collectAsStateWithLifecycle()
    val s = starred.value
    if (s == null) {
        LoadingOrError(starred.error, { vm.retry(LibraryTab.Albums, true) })
        return
    }
    val albums: List<Album> = remember(s, overrides, sort, articles) {
        sort.sortLocally(s.album.filter { Library.isFavourite(it.id, it.starred, overrides) }, articles, vm.randomSeed)
    }
    val songCount = s.song.count { Library.isFavourite(it.id, it.starred, overrides) }
    LazyVerticalGrid(
        columns = GridCells.Adaptive(AlbumCellMin),
        contentPadding = gridPadding,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
        modifier = Modifier.fillMaxSize(),
    ) {
        item(key = "songs", span = { GridItemSpan(maxLineSpan) }, contentType = "songs") {
            FavouriteSongsRow(songCount, onOpenSongs)
        }
        if (albums.isEmpty()) {
            item(key = "empty", span = { GridItemSpan(maxLineSpan) }) { Empty("No favourite albums yet.") }
        }
        items(albums, key = { it.id }, contentType = { "album" }) { a -> AlbumTile(a, onOpenAlbum) }
    }
}

/** "Downloads · 245 MB": the Downloads screen (state, sizes, removing). */
@Composable
private fun ManageDownloadsRow(onOpen: () -> Unit) {
    val palette = Palette.current
    val stored by Downloads.stored.collectAsStateWithLifecycle()
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .background(palette.surface, androidx.compose.foundation.shape.RoundedCornerShape(Tokens.RadiusCard))
            .clickable(role = Role.Button, onClickLabel = "Open downloads", onClick = onOpen)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(Icons.Rounded.DownloadForOffline, contentDescription = null, tint = palette.accent)
        Text("Downloads", style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
        Text(formatBytes(stored.totalBytes), style = MaterialTheme.typography.bodySmall, color = palette.textDim)
        Icon(Icons.Rounded.KeyboardArrowRight, contentDescription = null, tint = palette.textDim)
    }
}

/** "Favourite songs · 5": the starred songs open as a playlist-like page. */
@Composable
private fun FavouriteSongsRow(count: Int, onOpen: () -> Unit) {
    val palette = Palette.current
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .background(palette.surface, androidx.compose.foundation.shape.RoundedCornerShape(Tokens.RadiusCard))
            .clickable(role = Role.Button, onClickLabel = "Open favourite songs", onClick = onOpen)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(Icons.Rounded.Favorite, contentDescription = null, tint = palette.accent)
        Text("Favourite songs", style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
        Text(songCountLabel(count), style = MaterialTheme.typography.bodySmall, color = palette.textDim)
        Icon(Icons.Rounded.KeyboardArrowRight, contentDescription = null, tint = palette.textDim)
    }
}

/** The downloaded albums, in the current sort, from the pins alone (works offline). */
@Composable
private fun DownloadedAlbums(vm: LibraryViewModel, onOpenAlbum: (String) -> Unit, onManage: () -> Unit) {
    val pins by Downloads.pins.collectAsStateWithLifecycle()
    val sort by LibraryPrefs.sort.collectAsStateWithLifecycle()
    val articles by vm.articles.collectAsStateWithLifecycle()
    val albums = remember(pins, sort, articles) {
        sort.sortLocally(pins.pins.filter { it.kind == PinKind.Album }.map(Downloads::albumOf), articles, vm.randomSeed)
    }
    LazyVerticalGrid(
        columns = GridCells.Adaptive(AlbumCellMin),
        contentPadding = gridPadding,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
        modifier = Modifier.fillMaxSize(),
    ) {
        item(key = "manage", span = { GridItemSpan(maxLineSpan) }, contentType = "manage") { ManageDownloadsRow(onManage) }
        if (albums.isEmpty()) {
            item(key = "empty", span = { GridItemSpan(maxLineSpan) }) { Empty("No albums downloaded yet. Download one from its page or with a long press.") }
        }
        items(albums, key = { it.id }, contentType = { "album" }) { a -> AlbumTile(a, onOpenAlbum) }
    }
}

@Composable
private fun DownloadedPlaylists(onOpen: (String) -> Unit, onManage: () -> Unit) {
    val pins by Downloads.pins.collectAsStateWithLifecycle()
    val lists = remember(pins) { pins.pins.filter { it.kind == PinKind.Playlist }.map(Downloads::playlistOf).sortedBy { it.name.lowercase() } }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 4.dp, bottom = 24.dp)) {
        item(key = "manage") { Box(Modifier.padding(horizontal = Tokens.PageMargin, vertical = 4.dp)) { ManageDownloadsRow(onManage) } }
        if (lists.isEmpty()) item(key = "empty") { Empty("No playlists downloaded yet.") }
        items(lists, key = { it.id }, contentType = { "playlist" }) { p -> PlaylistRow(p, onOpen) }
    }
}

// ---- Artists -----------------------------------------------------------------------------------

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ArtistsList(vm: LibraryViewModel, favourites: Boolean, onOpenArtist: (String) -> Unit) {
    val palette = Palette.current
    val index by vm.artists.collectAsStateWithLifecycle()
    val starred by vm.starred.collectAsStateWithLifecycle()
    val overrides by Library.favourites.collectAsStateWithLifecycle()
    val list = rememberLazyListState()
    val scope = rememberCoroutineScope()
    var held by remember { mutableStateOf<String?>(null) }

    if (favourites) {
        val s = starred.value
        if (s == null) {
            LoadingOrError(starred.error, { vm.retry(LibraryTab.Artists, true) })
            return
        }
        val artists = remember(s, overrides) { s.artist.filter { Library.isFavourite(it.id, it.starred, overrides) }.sortedBy { it.name.lowercase() } }
        if (artists.isEmpty()) return Empty("No favourite artists yet.")
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 4.dp, bottom = 24.dp)) {
            items(artists, key = { it.id }, contentType = { "artist" }) { a -> ArtistRow(a, onOpenArtist) }
        }
        return
    }
    val idx = index.value
    if (idx == null) {
        LoadingOrError(index.error, { vm.retry(LibraryTab.Artists, false) })
        return
    }
    // Where each letter's header sits in the list, for the alphabet scroller.
    val sections = remember(idx) { idx.index.filter { it.artist.isNotEmpty() } }
    val headerAt = remember(sections) {
        var i = 0
        sections.map { s -> i.also { i += 1 + s.artist.size } }
    }
    Box(Modifier.fillMaxSize()) {
        LazyColumn(state = list, modifier = Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 4.dp, bottom = 24.dp, end = 20.dp)) {
            for (section in sections) {
                stickyHeader(key = "h-${section.name}", contentType = "letter") {
                    Text(
                        section.name,
                        style = MaterialTheme.typography.labelLarge,
                        color = palette.accent,
                        modifier = Modifier.fillMaxWidth().background(palette.bg).padding(horizontal = Tokens.PageMargin, vertical = 6.dp).semantics { heading() },
                    )
                }
                items(section.artist, key = { it.id }, contentType = { "artist" }) { a: Artist -> ArtistRow(a, onOpenArtist) }
            }
        }
        AlphabetScroller(
            letters = sections.map { it.name },
            onLetter = { i -> scope.launch { list.scrollToItem(headerAt[i]) } },
            onHeld = { held = it },
            modifier = Modifier.align(Alignment.CenterEnd).padding(vertical = 8.dp),
        )
        held?.let { LetterBubble(it, Modifier.align(Alignment.Center)) }
    }
}

// ---- Playlists and genres ------------------------------------------------------------------------

@Composable
private fun PlaylistsList(vm: LibraryViewModel, onOpen: (String) -> Unit) {
    val state by vm.playlists.collectAsStateWithLifecycle()
    // An addition made here shows in the counts at once.
    val changed by Library.playlistsChanged.collectAsStateWithLifecycle()
    val lists = changed ?: state.value
    if (lists == null) {
        LoadingOrError(state.error, { vm.retry(LibraryTab.Playlists, false) })
        return
    }
    if (lists.isEmpty()) return Empty("No playlists yet.")
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 4.dp, bottom = 24.dp)) {
        items(lists, key = { it.id }, contentType = { "playlist" }) { p -> PlaylistRow(p, onOpen) }
    }
}

/**
 * Genres by album count. Genres with no albums (tags only on single tracks) are left out: their
 * page would be empty.
 */
@Composable
private fun GenresList(vm: LibraryViewModel, onOpen: (Genre) -> Unit) {
    val palette = Palette.current
    val state by vm.genres.collectAsStateWithLifecycle()
    val all = state.value
    if (all == null) {
        LoadingOrError(state.error, { vm.retry(LibraryTab.Genres, false) })
        return
    }
    val genres = remember(all) { all.filter { it.albumCount > 0 }.sortedWith(compareByDescending<Genre> { it.albumCount }.thenByDescending { it.songCount }.thenBy { it.value.lowercase() }) }
    if (genres.isEmpty()) return Empty("No genres.")
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 4.dp, bottom = 24.dp)) {
        items(genres, key = { it.value }, contentType = { "genre" }) { g ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 56.dp)
                    .clickable(role = Role.Button, onClickLabel = "Open genre") { onOpen(g) }
                    .padding(horizontal = Tokens.PageMargin, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(g.value, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                Text(
                    listOf(albumsLabel(g.albumCount), songCountLabel(g.songCount)).joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = palette.textDim,
                )
            }
        }
    }
}

@Composable
private fun Empty(text: String) {
    Text(text, color = Palette.current.textDim, modifier = Modifier.padding(Tokens.PageMargin))
}
