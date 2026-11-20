package com.offbeat.marquee.player.ui.actions

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.PlaylistAdd
import androidx.compose.material.icons.automirrored.rounded.QueueMusic
import androidx.compose.material.icons.rounded.Album
import androidx.compose.material.icons.rounded.DownloadForOffline
import androidx.compose.material.icons.rounded.Favorite
import androidx.compose.material.icons.rounded.FavoriteBorder
import androidx.compose.material.icons.rounded.Person
import androidx.compose.material.icons.rounded.PlaylistPlay
import androidx.compose.material.icons.rounded.RemoveCircleOutline
import androidx.compose.material.icons.rounded.Shuffle
import androidx.compose.material.icons.rounded.Star
import androidx.compose.material.icons.rounded.StarBorder
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.offbeat.marquee.core.library.ActionPlace
import com.offbeat.marquee.core.library.ItemAction
import com.offbeat.marquee.core.library.ItemActions
import com.offbeat.marquee.core.subsonic.Album as AlbumModel
import com.offbeat.marquee.core.subsonic.Artist
import com.offbeat.marquee.core.subsonic.Playlist
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.data.Item
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.core.offline.Pin
import com.offbeat.marquee.core.offline.PinKind
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.ui.components.pinProgress
import com.offbeat.marquee.player.playback.FavouriteResult
import com.offbeat.marquee.player.playback.PlayerConnection
import com.offbeat.marquee.player.session.errorMessage
import com.offbeat.marquee.player.ui.components.formatLength
import com.offbeat.marquee.player.ui.components.DarkSheetBars
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json

/**
 * A long press: the item, where it happened, and for queue rows what Play next and Remove do there
 * (moving and removing the entry rather than adding the song again). [queueUid] names the queue
 * entry; [withQueueCallbacks] builds those two from it, so they act on the queue as it is when
 * tapped (it can change while the sheet is open) and survive a rotation.
 */
@Immutable
data class ActionRequest(
    val item: Item,
    val place: ActionPlace = ActionPlace(),
    val playNextInQueue: (() -> Unit)? = null,
    val removeFromQueue: (() -> Unit)? = null,
    val queueUid: Long? = null,
) {
    /** A queue row's callbacks built from [queueUid], acting on the queue as it is when tapped. */
    fun withQueueCallbacks(player: PlayerConnection): ActionRequest {
        val uid = queueUid ?: return this
        return copy(
            playNextInQueue = {
                val queue = player.queue.value
                val current = queue.indexOfFirst { it.index == player.state.value.index }.coerceAtLeast(0)
                queue.firstOrNull { it.uid == uid }?.let { row ->
                    player.moveInQueue(row.position, if (row.position < current) current else current + 1)
                }
            },
            removeFromQueue = { player.queue.value.firstOrNull { it.uid == uid }?.let { player.removeFromQueue(it.index) } },
        )
    }
}

/**
 * Keeps the open sheet across a configuration change (rotation): its subject (the item's kind and
 * model, without its track lists, which the actions fetch again by id), where it was opened, and the
 * queue entry; not the callbacks, which [ActionRequest.withQueueCallbacks] rebuilds.
 */
val ActionRequestSaver: Saver<ActionRequest?, Any> = Saver(
    save = { r ->
        r?.let {
            val (kind, model) = when (val item = it.item) {
                is Item.OfAlbum -> "album" to saverJson.encodeToString(AlbumModel.serializer(), item.album.copy(song = emptyList()))
                is Item.OfSong -> "song" to saverJson.encodeToString(Song.serializer(), item.song)
                is Item.OfArtist -> "artist" to saverJson.encodeToString(Artist.serializer(), item.artist.copy(album = emptyList()))
                is Item.OfPlaylist -> "playlist" to saverJson.encodeToString(Playlist.serializer(), item.playlist.copy(entry = emptyList()))
            }
            arrayListOf(kind, model, it.place.inQueue, it.place.isCurrent, it.place.onOwnPage, it.queueUid ?: NO_UID)
        }
    },
    restore = { saved ->
        runCatching {
            val l = saved as List<*>
            val model = l[1] as String
            val item = when (l[0]) {
                "album" -> Item.OfAlbum(saverJson.decodeFromString(AlbumModel.serializer(), model))
                "song" -> Item.OfSong(saverJson.decodeFromString(Song.serializer(), model))
                "artist" -> Item.OfArtist(saverJson.decodeFromString(Artist.serializer(), model))
                "playlist" -> Item.OfPlaylist(saverJson.decodeFromString(Playlist.serializer(), model))
                else -> return@runCatching null
            }
            ActionRequest(
                item = item,
                place = ActionPlace(inQueue = l[2] as Boolean, isCurrent = l[3] as Boolean, onOwnPage = l[4] as Boolean),
                queueUid = (l[5] as Long).takeIf { it != NO_UID },
            )
        }.getOrNull()
    },
)

private const val NO_UID = Long.MIN_VALUE
private val saverJson = Json { ignoreUnknownKeys = true }

/** Opens the one long-press sheet (provided by the scaffold). */
val LocalItemActions = staticCompositionLocalOf<(ActionRequest) -> Unit> { {} }

/**
 * Tap to open, long press for the action sheet (with a haptic tick). The one gesture every album,
 * track, artist and playlist uses.
 */
@OptIn(ExperimentalFoundationApi::class)
fun Modifier.itemGestures(
    item: Item,
    onClick: () -> Unit,
    onClickLabel: String? = null,
    place: ActionPlace = ActionPlace(),
    role: Role? = Role.Button,
): Modifier = composed {
    val open = LocalItemActions.current
    val haptics = LocalHapticFeedback.current
    combinedClickable(
        role = role,
        onClickLabel = onClickLabel,
        onLongClickLabel = "More actions",
        onLongClick = {
            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
            open(ActionRequest(item, place))
        },
        onClick = onClick,
    )
}

/** Where the sheet's "Go to" actions lead; [beforeNavigate] closes Now Playing when the press came from its queue. */
class ActionNavigation(
    val openAlbum: (String) -> Unit,
    val openArtist: (String) -> Unit,
    val beforeNavigate: () -> Unit,
)

/**
 * The long-press sheet: the item on top, then what can be done with it ([ItemActions.applicable]).
 * Writes (favourite, rate, add to playlist) go through [Library], which refuses them in read-only
 * builds with a snackbar and otherwise shows them at once, rolling back on error.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ActionSheet(
    request: ActionRequest,
    player: PlayerConnection,
    playingSongId: String,
    nav: ActionNavigation,
    scope: CoroutineScope,
    onDismiss: () -> Unit,
) {
    val palette = Palette.current
    val item = request.item
    val favourites by Library.favourites.collectAsStateWithLifecycle()
    val ratings by Library.ratings.collectAsStateWithLifecycle()
    var picking by remember(request) { mutableStateOf(false) }
    val (hasAlbum, hasArtist) = when (item) {
        is Item.OfSong -> (item.song.albumId != null) to (item.song.artistId != null)
        is Item.OfAlbum -> true to (item.album.artistId != null)
        is Item.OfArtist -> false to true
        is Item.OfPlaylist -> false to false
    }
    // A queue row's sheet can stay open while the track changes: whether it is the current one is read live.
    val nowIndex = player.state.collectAsStateWithLifecycle().value.index
    val queue by player.queue.collectAsStateWithLifecycle()
    val place = request.queueUid?.let { uid ->
        queue.firstOrNull { it.uid == uid }?.let { request.place.copy(isCurrent = it.index == nowIndex) }
    } ?: request.place
    val actions = ItemActions.applicable(item.kind, hasAlbum, hasArtist, place)
    val starred = when (item) {
        is Item.OfSong -> item.song.starred
        is Item.OfAlbum -> item.album.starred
        is Item.OfArtist -> item.artist.starred
        is Item.OfPlaylist -> null
    }
    val favourite = Library.isFavourite(item.id, starred, favourites)
    val userRating = when (item) {
        is Item.OfSong -> item.song.userRating
        is Item.OfAlbum -> item.album.userRating
        else -> null
    }
    val rating = Library.rating(item.id, userRating, ratings)
    val pins by Downloads.pins.collectAsStateWithLifecycle()
    val pin = pinKey(item)?.let { pins[it] }
    val pinned = pin != null
    val pinNote = pinProgress(pin)?.let { p -> if (p.complete) "Downloaded" else "${p.label} · ${(p.fraction * 100).toInt()}%" }

    fun withSongs(shuffled: Boolean = false, then: (List<Song>) -> Unit) {
        scope.launch {
            try {
                val songs = Library.songsOf(item, shuffled)
                if (songs.isEmpty()) Library.post("Nothing to play") else then(songs)
            } catch (e: SubsonicError) {
                Library.post(errorMessage(e))
            }
        }
    }

    fun run(action: ItemAction) {
        when (action) {
            ItemAction.PlayNext -> {
                val inQueue = request.playNextInQueue
                if (inQueue != null) inQueue() else withSongs { player.enqueue(it, next = true); Library.post(if (it.size == 1) "Playing next" else "${it.size} songs playing next") }
            }
            ItemAction.AddToQueue -> withSongs { player.enqueue(it, next = false); Library.post(if (it.size == 1) "Added to the queue" else "${it.size} songs added to the queue") }
            ItemAction.Shuffle -> withSongs(shuffled = true) { player.playSongs(it, 0) }
            ItemAction.GoToAlbum -> {
                val id = when (item) {
                    is Item.OfSong -> item.song.albumId
                    is Item.OfAlbum -> item.album.id
                    else -> null
                }
                id?.let { nav.beforeNavigate(); nav.openAlbum(it) }
            }
            ItemAction.GoToArtist -> {
                val id = when (item) {
                    is Item.OfSong -> item.song.artistId
                    is Item.OfAlbum -> item.album.artistId
                    is Item.OfArtist -> item.artist.id
                    else -> null
                }
                id?.let { nav.beforeNavigate(); nav.openArtist(it) }
            }
            ItemAction.Favourite -> scope.launch {
                if (item is Item.OfSong && item.song.id == playingSongId) {
                    // The playing song: through the service, so the notification's heart follows.
                    when (player.toggleFavourite()) {
                        FavouriteResult.ReadOnly -> Library.postReadOnly()
                        FavouriteResult.Failed -> Library.post("Couldn't change the favourite")
                        FavouriteResult.Done -> Unit
                    }
                } else {
                    Library.setFavourite(item, !favourite)
                }
            }
            ItemAction.Rate -> Unit // the stars do it
            ItemAction.AddToPlaylist -> {
                picking = true
                return
            }
            ItemAction.Download -> scope.launch {
                val key = pinKey(item)
                if (key != null && pins[key] != null) Downloads.unpin(key) else Downloads.pin(item)
            }
            ItemAction.RemoveFromQueue -> request.removeFromQueue?.invoke()
        }
        onDismiss()
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = palette.surface,
    ) {
        DarkSheetBars()
        Column(Modifier.navigationBarsPadding().padding(bottom = 8.dp)) {
            if (picking) {
                PlaylistPicker(
                    onBack = { picking = false },
                    onPick = { playlist ->
                        onDismiss()
                        withSongs { songs -> scope.launch { Library.addToPlaylist(playlist, songs) } }
                    },
                )
            } else {
                Header(item)
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    for (action in actions) {
                        when (action) {
                            ItemAction.Rate -> RateRow(rating) { n ->
                                scope.launch { Library.setRating(item, ItemActions.nextRating(rating, n)) }
                                onDismiss()
                            }
                            else -> ActionRow(
                                icon = iconOf(action, favourite),
                                label = if (action == ItemAction.Download && pinned) "Remove download" else labelOf(action, favourite),
                                note = if (action == ItemAction.Download && pinned) pinNote else null,
                                onClick = { run(action) },
                            )
                        }
                    }
                }
            }
        }
    }
}

/** The pin a Download in the sheet adds or removes. */
private fun pinKey(item: Item): String? = when (item) {
    is Item.OfAlbum -> Pin.keyOf(PinKind.Album, item.id)
    is Item.OfPlaylist -> Pin.keyOf(PinKind.Playlist, item.id)
    is Item.OfSong -> Pin.keyOf(PinKind.Song, item.id)
    is Item.OfArtist -> null
}

private fun labelOf(action: ItemAction, favourite: Boolean): String = when (action) {
    ItemAction.Favourite -> if (favourite) "Remove from favourites" else "Add to favourites"
    ItemAction.AddToPlaylist -> "Add to playlist…"
    else -> action.label
}

private fun iconOf(action: ItemAction, favourite: Boolean): ImageVector = when (action) {
    ItemAction.PlayNext -> Icons.Rounded.PlaylistPlay
    ItemAction.AddToQueue -> Icons.AutoMirrored.Rounded.QueueMusic
    ItemAction.Shuffle -> Icons.Rounded.Shuffle
    ItemAction.GoToAlbum -> Icons.Rounded.Album
    ItemAction.GoToArtist -> Icons.Rounded.Person
    ItemAction.Favourite -> if (favourite) Icons.Rounded.Favorite else Icons.Rounded.FavoriteBorder
    ItemAction.Rate -> Icons.Rounded.Star
    ItemAction.AddToPlaylist -> Icons.AutoMirrored.Rounded.PlaylistAdd
    ItemAction.Download -> Icons.Rounded.DownloadForOffline
    ItemAction.RemoveFromQueue -> Icons.Rounded.RemoveCircleOutline
}

@Composable
private fun Header(item: Item) {
    val palette = Palette.current
    val (cover, title, sub) = when (item) {
        is Item.OfSong -> Triple(item.song.coverArt, item.song.title, listOfNotNull(item.song.artist, item.song.album).joinToString(" · "))
        is Item.OfAlbum -> Triple(item.album.coverArt, item.album.name, listOfNotNull(item.album.artist, item.album.year?.takeIf { it > 0 }?.toString()).joinToString(" · "))
        is Item.OfArtist -> Triple(item.artist.coverArt, item.artist.name, albumsLabel(item.artist.albumCount))
        is Item.OfPlaylist -> Triple(item.playlist.coverArt, item.playlist.name, songsLabel(item.playlist.songCount, item.playlist.duration))
    }
    val shape: Shape = if (item is Item.OfArtist) CircleShape else RoundedCornerShape(6.dp)
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        CoverImage(cover, Modifier.size(56.dp), shape = shape)
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.semantics { heading() })
            if (sub.isNotEmpty()) Text(sub, style = MaterialTheme.typography.bodySmall, color = palette.textDim, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun ActionRow(icon: ImageVector, label: String, note: String?, onClick: () -> Unit) {
    val palette = Palette.current
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 20.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        Icon(icon, contentDescription = null, tint = palette.textDim)
        Column(Modifier.weight(1f)) {
            Text(label, style = MaterialTheme.typography.bodyLarge, color = palette.text)
            if (note != null) Text(note, style = MaterialTheme.typography.bodySmall, color = palette.textFaint)
        }
    }
}

/**
 * Five stars: tapping one rates, tapping the current one clears the rating. The label keeps its
 * width (it never wraps); the stars sit at the end of the row, and when the font is so large that
 * both do not fit on one line, the stars go on the next one.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun RateRow(rating: Int, onRate: (Int) -> Unit) {
    val palette = Palette.current
    FlowRow(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).padding(start = 20.dp, end = 12.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalArrangement = Arrangement.Center,
    ) {
        Row(Modifier.align(Alignment.CenterVertically).heightIn(min = 52.dp).padding(end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Rounded.Star, contentDescription = null, tint = palette.textDim)
            Spacer(Modifier.width(18.dp))
            Text("Rate", style = MaterialTheme.typography.bodyLarge, color = palette.text, maxLines = 1, softWrap = false)
        }
        Row(Modifier.align(Alignment.CenterVertically), verticalAlignment = Alignment.CenterVertically) {
            for (n in 1..5) {
                val lit = n <= rating
                IconButton(
                    onClick = { onRate(n) },
                    modifier = Modifier.size(48.dp).semantics {
                        contentDescription = if (n == rating) "$n stars, tap to clear" else if (n == 1) "1 star" else "$n stars"
                        selected = n == rating
                    },
                ) {
                    Icon(if (lit) Icons.Rounded.Star else Icons.Rounded.StarBorder, contentDescription = null, tint = if (lit) palette.accent else palette.textFaint)
                }
            }
        }
    }
}

/** The user's playlists that can take songs (not smart playlists, not other users'). */
@Composable
private fun PlaylistPicker(onBack: () -> Unit, onPick: (Playlist) -> Unit) {
    val palette = Palette.current
    var lists by remember { mutableStateOf<List<Playlist>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        lists = Library.playlistsChanged.value ?: Library.cachedPlaylists()
        try {
            lists = Library.fetchPlaylists()
        } catch (e: SubsonicError) {
            if (lists == null) error = errorMessage(e)
        }
    }
    Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = onBack, modifier = Modifier.size(48.dp)) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = "Back") }
        Text("Add to playlist", style = MaterialTheme.typography.titleMedium, modifier = Modifier.semantics { heading() })
    }
    val editable = lists?.let(Library::editable)
    when {
        editable == null -> Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) {
            if (error != null) Text(error.orEmpty(), color = MaterialTheme.colorScheme.error) else CircularProgressIndicator()
        }
        editable.isEmpty() -> Text("No playlists of yours to add to.", color = palette.textDim, modifier = Modifier.padding(20.dp))
        else -> LazyColumn(Modifier.heightIn(max = 420.dp)) {
            items(editable, key = { it.id }) { p ->
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 60.dp).clickable(role = Role.Button) { onPick(p) }.padding(horizontal = 20.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    CoverImage(p.coverArt, Modifier.size(44.dp), shape = RoundedCornerShape(6.dp))
                    Column(Modifier.weight(1f)) {
                        Text(p.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(songsLabel(p.songCount, p.duration), style = MaterialTheme.typography.bodySmall, color = palette.textDim)
                    }
                }
            }
        }
    }
}

fun albumsLabel(n: Int): String = if (n == 1) "1 album" else "$n albums"

fun songCountLabel(n: Int): String = if (n == 1) "1 song" else "$n songs"

fun songsLabel(count: Int, seconds: Int): String =
    listOfNotNull(songCountLabel(count), seconds.takeIf { it > 0 }?.let(::formatLength)).joinToString(" · ")
