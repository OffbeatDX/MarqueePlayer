package com.offbeat.marquee.player.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material.icons.rounded.DownloadDone
import androidx.compose.runtime.getValue
import androidx.compose.ui.draw.alpha
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.offbeat.marquee.core.offline.PinKind
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.offline.LocalOffline
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.offbeat.marquee.core.library.ActionPlace
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.Artist
import com.offbeat.marquee.core.subsonic.Playlist
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.data.Item
import com.offbeat.marquee.player.ui.LocalPlaying
import com.offbeat.marquee.player.ui.actions.albumsLabel
import com.offbeat.marquee.player.ui.actions.itemGestures
import com.offbeat.marquee.player.ui.actions.songsLabel
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens

/**
 * An album as a cover with its title and artist under it, for rows and grids. Long press opens
 * the action sheet; the playing album's title is in the accent with the playing mark, as on the
 * album page.
 */
@Composable
fun AlbumTile(album: Album, onOpen: (String) -> Unit, modifier: Modifier = Modifier, place: ActionPlace = ActionPlace()) {
    val palette = Palette.current
    val playing = LocalPlaying.current.value.albumId == album.id
    val pins by Downloads.pins.collectAsStateWithLifecycle()
    // Offline, an album that isn't downloaded is dimmed (it still opens: some songs may be cached).
    val dim = !LocalOffline.current.online && !pins.isPinned(PinKind.Album, album.id)
    Column(
        modifier
            .alpha(if (dim) 0.45f else 1f)
            .clip(RoundedCornerShape(Tokens.RadiusCover))
            .itemGestures(Item.OfAlbum(album), onClick = { onOpen(album.id) }, onClickLabel = "Open album", place = place)
            .padding(bottom = 6.dp),
    ) {
        CoverImage(
            coverId = album.coverArt,
            modifier = Modifier.fillMaxWidth().aspectRatio(1f),
            contentDescription = null,
        )
        Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            if (playing) {
                Icon(Icons.Rounded.GraphicEq, contentDescription = "Playing", tint = palette.accent, modifier = Modifier.size(16.dp))
                Spacer(Modifier.width(4.dp))
            }
            Text(
                album.name,
                style = MaterialTheme.typography.titleSmall,
                color = if (playing) palette.accent else palette.text,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Text(
            album.artist.orEmpty(),
            style = MaterialTheme.typography.bodySmall,
            color = palette.textDim,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** A grid cell whose album is not fetched yet: the tile's shape, quiet, so nothing moves when it arrives. */
@Composable
fun AlbumPlaceholderTile(modifier: Modifier = Modifier) {
    val palette = Palette.current
    Column(modifier.padding(bottom = 6.dp)) {
        Box(Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(Tokens.RadiusCover)).background(palette.surface))
        Box(Modifier.padding(top = 10.dp).fillMaxWidth(0.7f).height(12.dp).clip(RoundedCornerShape(4.dp)).background(palette.surface))
        Box(Modifier.padding(top = 8.dp).fillMaxWidth(0.45f).height(10.dp).clip(RoundedCornerShape(4.dp)).background(palette.surface))
    }
}

/** What a song row shows on its left. */
sealed interface SongLead {
    /** The track number (album pages). */
    data class Number(val n: Int?) : SongLead

    /** The song's cover (playlists, search, favourites). */
    data object Cover : SongLead
}

/**
 * A song: title, artist (and album when [showAlbum]), length; tap plays, long press opens the
 * sheet. The playing song is in the accent with the playing mark.
 */
@Composable
fun SongRow(
    song: Song,
    lead: SongLead,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    showAlbum: Boolean = false,
    place: ActionPlace = ActionPlace(),
) {
    val palette = Palette.current
    val current = LocalPlaying.current.value.songId == song.id
    val offline = LocalOffline.current
    // Offline, a song that is neither downloaded nor whole in the cache is dimmed and does not play.
    val playable = current || offline.canPlay(song.id)
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .alpha(if (playable) 1f else 0.38f)
            .itemGestures(
                Item.OfSong(song),
                onClick = if (playable) onClick else ({ Library.post("Not available offline") }),
                onClickLabel = if (playable) "Play from here" else null,
                place = place,
            )
            .padding(horizontal = Tokens.PageMargin, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        when (lead) {
            is SongLead.Number -> Box(Modifier.width(28.dp), contentAlignment = Alignment.CenterStart) {
                if (current) {
                    Icon(Icons.Rounded.GraphicEq, contentDescription = "Playing", tint = palette.accent, modifier = Modifier.size(20.dp))
                } else {
                    Text(lead.n?.toString() ?: "·", color = palette.textFaint, style = MaterialTheme.typography.bodyMedium)
                }
            }
            SongLead.Cover -> CoverImage(song.coverArt, Modifier.size(44.dp), shape = RoundedCornerShape(6.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(
                song.title,
                style = MaterialTheme.typography.bodyLarge,
                color = if (current) palette.accent else palette.text,
                fontWeight = if (current) FontWeight.SemiBold else FontWeight.Normal,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            val sub = if (showAlbum) listOfNotNull(song.artist, song.album).joinToString(" · ") else song.artist.orEmpty()
            if (sub.isNotEmpty()) {
                Text(sub, style = MaterialTheme.typography.bodySmall, color = palette.textDim, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (current && lead == SongLead.Cover) {
            Icon(Icons.Rounded.GraphicEq, contentDescription = "Playing", tint = palette.accent, modifier = Modifier.size(20.dp))
        }
        if (offline.isDownloaded(song.id)) {
            Icon(Icons.Rounded.DownloadDone, contentDescription = "Downloaded", tint = palette.textDim, modifier = Modifier.size(16.dp))
        }
        Text(formatTime((song.duration ?: 0) * 1000L), color = palette.textDim, style = MaterialTheme.typography.bodySmall)
    }
}

/** An artist: round picture, name, album count. */
@Composable
fun ArtistRow(artist: Artist, onOpen: (String) -> Unit, modifier: Modifier = Modifier) {
    val palette = Palette.current
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = 60.dp)
            .itemGestures(Item.OfArtist(artist), onClick = { onOpen(artist.id) }, onClickLabel = "Open artist")
            .padding(horizontal = Tokens.PageMargin, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        CoverImage(artist.coverArt, Modifier.size(44.dp), shape = CircleShape)
        Column(Modifier.weight(1f)) {
            Text(artist.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (artist.albumCount > 0) {
                Text(albumsLabel(artist.albumCount), style = MaterialTheme.typography.bodySmall, color = palette.textDim)
            }
        }
    }
}

/** A playlist: cover, name, song count and length. */
@Composable
fun PlaylistRow(playlist: Playlist, onOpen: (String) -> Unit, modifier: Modifier = Modifier) {
    val palette = Palette.current
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = 72.dp)
            .itemGestures(Item.OfPlaylist(playlist), onClick = { onOpen(playlist.id) }, onClickLabel = "Open playlist")
            .padding(horizontal = Tokens.PageMargin, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        CoverImage(playlist.coverArt, Modifier.size(56.dp), shape = RoundedCornerShape(Tokens.RadiusCover))
        Column(Modifier.weight(1f)) {
            Text(playlist.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(songsLabel(playlist.songCount, playlist.duration), style = MaterialTheme.typography.bodySmall, color = palette.textDim)
        }
    }
}
