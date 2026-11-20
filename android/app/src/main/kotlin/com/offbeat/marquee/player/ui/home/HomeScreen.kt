package com.offbeat.marquee.player.ui.home

import com.offbeat.marquee.player.BuildConfig
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.Shuffle
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.min
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.core.library.ShelfId
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.player.art.Backdrop
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.data.Item
import com.offbeat.marquee.player.ui.actions.itemGestures
import com.offbeat.marquee.player.session.SessionState
import com.offbeat.marquee.player.ui.components.AccentButton
import com.offbeat.marquee.player.ui.components.AlbumTile
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.components.LocalWide
import com.offbeat.marquee.player.ui.components.LoadingOrError
import com.offbeat.marquee.player.ui.components.QuietButton
import com.offbeat.marquee.player.ui.components.ReadOnlyBadge
import com.offbeat.marquee.player.ui.components.SectionHeader
import com.offbeat.marquee.player.ui.rememberPlayAlbum
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    session: SessionState.SignedIn,
    onOpenAlbum: (String) -> Unit,
    onOpenArtist: (String) -> Unit,
    onSeeAll: (ShelfId) -> Unit,
    onSettings: () -> Unit,
    vm: HomeViewModel = viewModel { HomeViewModel() },
) {
    val state by vm.state.collectAsStateWithLifecycle()
    val playAlbum = rememberPlayAlbum()
    val wide = LocalWide.current
    PullToRefreshBox(isRefreshing = state.refreshing, onRefresh = vm::refresh, modifier = Modifier.fillMaxSize()) {
        LazyColumn(
            contentPadding = PaddingValues(top = LocalScreenTop.current, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(20.dp),
            modifier = Modifier.fillMaxSize(),
        ) {
            item(key = "top", contentType = "top") { TopBar(session, onSettings) }
            if (state.empty) {
                item(key = "loading", contentType = "loading") {
                    LoadingOrError(state.error, vm::refresh, Modifier.height(320.dp))
                }
            } else if (state.shelves.all { it.second.isEmpty() }) {
                item(key = "nothing", contentType = "loading") {
                    Text(
                        "No albums yet.",
                        color = Palette.current.textDim,
                        modifier = Modifier.padding(horizontal = Tokens.PageMargin),
                    )
                }
            }
            if (!state.empty && state.error != null) {
                item(key = "offline", contentType = "note") {
                    Text(
                        state.error.orEmpty(),
                        style = MaterialTheme.typography.bodySmall,
                        color = Palette.current.textDim,
                        modifier = Modifier.padding(horizontal = Tokens.PageMargin),
                    )
                }
            }
            for ((shelf, albums) in state.shelves) {
                if (albums.isEmpty()) continue
                if (shelf.isHero) {
                    item(key = shelf.key, contentType = "hero") {
                        HeroCard(
                            shelf = shelf,
                            albums = albums,
                            featured = state.hero.coerceIn(0, albums.lastIndex),
                            wide = wide,
                            onFeature = vm::feature,
                            onOpenAlbum = onOpenAlbum,
                            onOpenArtist = onOpenArtist,
                            onPlay = playAlbum,
                            onSeeAll = { onSeeAll(shelf) },
                        )
                    }
                } else {
                    item(key = shelf.key, contentType = "row") {
                        ShelfRow(
                            shelf = shelf,
                            albums = albums,
                            wide = wide,
                            onOpenAlbum = onOpenAlbum,
                            onSeeAll = { onSeeAll(shelf) },
                            onReshuffle = if (shelf.isRandom) vm::reshuffle else null,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun TopBar(session: SessionState.SignedIn, onSettings: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(start = Tokens.PageMargin, end = 4.dp, top = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            "Home",
            style = MaterialTheme.typography.headlineMedium,
            modifier = Modifier.semantics { heading() },
        )
        Spacer(Modifier.width(12.dp))
        // Test builds only (a build-time constant: the release build has no badge at all). It gets
        // what is left and ellipsizes, so a large font never squeezes the settings button.
        Box(Modifier.weight(1f)) {
            if (BuildConfig.READ_ONLY && session.readOnly) ReadOnlyBadge(session.testServer)
        }
        IconButton(onClick = onSettings, modifier = Modifier.size(48.dp)) {
            Icon(Icons.Rounded.Settings, contentDescription = "Settings")
        }
    }
}

/**
 * Recently added as the desktop's hero card: the featured album large over its blurred art, and
 * the shelf's other albums in a strip inside the card; choosing one features it.
 */
@Composable
private fun HeroCard(
    shelf: ShelfId,
    albums: List<Album>,
    featured: Int,
    wide: Boolean,
    onFeature: (Int) -> Unit,
    onOpenAlbum: (String) -> Unit,
    onOpenArtist: (String) -> Unit,
    onPlay: (String, Boolean) -> Unit,
    onSeeAll: () -> Unit,
) {
    val album = albums[featured]
    val palette = Palette.current
    // Wide windows are often short (a phone on its side): the cover follows the height.
    val height = LocalConfiguration.current.screenHeightDp.dp
    val coverSize = if (wide) min(200.dp, height * 0.36f) else 128.dp
    val strip = @Composable {
        if (albums.size > 1) HeroStrip(albums, featured, onFeature)
    }
    Box(
        Modifier
            .padding(horizontal = Tokens.PageMargin)
            .fillMaxWidth()
            .clip(RoundedCornerShape(Tokens.RadiusCard))
            .background(palette.surface),
    ) {
        Backdrop(album.coverArt, tint = Color.Transparent, modifier = Modifier.matchParentSize())
        Box(
            Modifier
                .matchParentSize()
                .background(Brush.horizontalGradient(listOf(Color(0xB0000000), Color(0x40000000)))),
        )
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                CoverImage(
                    coverId = album.coverArt,
                    modifier = Modifier
                        .size(coverSize)
                        .itemGestures(Item.OfAlbum(album), onClick = { onOpenAlbum(album.id) }, onClickLabel = "Open album"),
                    contentDescription = album.name,
                )
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    // The label opens the shelf's grid: a full 48 dp touch target around the small text.
                    Box(
                        Modifier
                            .heightIn(min = 48.dp)
                            .clickable(role = Role.Button, onClickLabel = "See all of ${shelf.title}", onClick = onSeeAll)
                            .padding(end = 12.dp),
                        contentAlignment = Alignment.CenterStart,
                    ) {
                        Text(shelf.title.uppercase(), style = MaterialTheme.typography.labelSmall, color = palette.accent)
                    }
                    Text(
                        album.name,
                        style = if (wide) MaterialTheme.typography.headlineMedium else MaterialTheme.typography.headlineSmall,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.clickable(onClickLabel = "Open album") { onOpenAlbum(album.id) },
                    )
                    Text(
                        album.artist.orEmpty(),
                        style = MaterialTheme.typography.bodyMedium,
                        color = palette.textDim,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier
                            .clickable(enabled = album.artistId != null, onClickLabel = "Open artist") { album.artistId?.let(onOpenArtist) }
                            .padding(vertical = 4.dp),
                    )
                    if (wide) {
                        HeroButtons(album, onPlay)
                        Spacer(Modifier.height(6.dp))
                        strip()
                    }
                }
            }
            if (!wide) {
                HeroButtons(album, onPlay)
                strip()
            }
        }
    }
}

/** The hero's other albums, small; the featured one ringed. */
@Composable
private fun HeroStrip(albums: List<Album>, featured: Int, onFeature: (Int) -> Unit) {
    val palette = Palette.current
    LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        itemsIndexed(albums, key = { _, a -> a.id }) { i, a ->
            val current = i == featured
            CoverImage(
                coverId = a.coverArt,
                modifier = Modifier
                    .size(52.dp)
                    .then(if (current) Modifier.border(BorderStroke(2.dp, palette.text), RoundedCornerShape(Tokens.RadiusCover)) else Modifier)
                    .itemGestures(Item.OfAlbum(a), onClick = { onFeature(i) }, onClickLabel = "Feature")
                    .semantics {
                        contentDescription = "${a.name}, ${a.artist.orEmpty()}"
                        selected = current
                    },
            )
        }
    }
}

@Composable
private fun HeroButtons(album: Album, onPlay: (String, Boolean) -> Unit) {
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.padding(top = 4.dp)) {
        AccentButton("Play", Icons.Rounded.PlayArrow, onClick = { onPlay(album.id, false) })
        QuietButton("Shuffle", Icons.Rounded.Shuffle, onClick = { onPlay(album.id, true) })
    }
}

/** One shelf: its header and a sideways row of albums. */
@Composable
private fun ShelfRow(
    shelf: ShelfId,
    albums: List<Album>,
    wide: Boolean,
    onOpenAlbum: (String) -> Unit,
    onSeeAll: () -> Unit,
    onReshuffle: (() -> Unit)?,
) {
    Column {
        SectionHeader(
            shelf.title,
            onOpen = onSeeAll,
            modifier = Modifier.padding(start = Tokens.PageMargin, end = 8.dp),
            extra = {
                if (onReshuffle != null) {
                    IconButton(onClick = onReshuffle, modifier = Modifier.size(48.dp)) {
                        Icon(Icons.Rounded.Shuffle, contentDescription = "Reshuffle ${shelf.title}", tint = Palette.current.textDim)
                    }
                }
            },
        )
        LazyRow(
            contentPadding = PaddingValues(horizontal = Tokens.PageMargin),
            horizontalArrangement = Arrangement.spacedBy(14.dp),
            modifier = Modifier.padding(top = 6.dp),
        ) {
            items(albums, key = { it.id }, contentType = { "album" }) { a ->
                AlbumTile(a, onOpenAlbum, Modifier.width(if (wide) 160.dp else 140.dp))
            }
        }
    }
}
