package com.offbeat.marquee.player.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.playback.PlayerConnection
import com.offbeat.marquee.player.session.errorMessage
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/** The UI's one connection to the player. */
val LocalPlayer = staticCompositionLocalOf<PlayerConnection> { error("No player") }

/** The song and album playing (or paused); empty when nothing is queued. */
@Immutable
data class PlayingIds(val songId: String = "", val albumId: String = "")

/**
 * What is playing, for marking it in lists and grids in the accent. A State, so only the cells that
 * read it recompose when the song changes.
 */
val LocalPlaying = staticCompositionLocalOf<State<PlayingIds>> { mutableStateOf(PlayingIds()) }

/** The playing ids from the player, collected while the UI is started (provided once, by the scaffold). */
@Composable
fun rememberPlayingIds(player: PlayerConnection): State<PlayingIds> {
    val flow = remember(player) {
        player.state.map { if (it.hasItem) PlayingIds(it.songId, it.albumId.orEmpty()) else PlayingIds() }.distinctUntilChanged()
    }
    return flow.collectAsStateWithLifecycle(PlayingIds())
}

/** The id of the song playing (or paused), for marking it in lists; recomposes only when it changes. */
@Composable
fun playingSongId(): State<String> {
    val playing = LocalPlaying.current
    return remember(playing) { androidx.compose.runtime.derivedStateOf { playing.value.songId } }
}

/** Plays an album by id (from the cache when it is there), in order or shuffled. */
@Composable
fun rememberPlayAlbum(): (albumId: String, shuffled: Boolean) -> Unit {
    val player = LocalPlayer.current
    val scope = rememberCoroutineScope()
    return remember(player, scope) {
        { id, shuffled ->
            scope.launch {
                try {
                    val songs: List<Song> = Library.album(id).song
                    player.playSongs(songs, 0, shuffled)
                } catch (e: SubsonicError) {
                    Library.post(errorMessage(e))
                }
            }
        }
    }
}
