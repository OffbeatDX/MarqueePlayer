package com.offbeat.marquee.core.subsonic

import kotlinx.serialization.Serializable

// Subsonic 1.16.1 + OpenSubsonic models, as far as the app uses them. Unknown fields are ignored,
// so each model lists only what is read somewhere.

/** The envelope fields every response carries; `ping` returns only these. */
@Serializable
data class ServerInfo(
    val status: String = "",
    val version: String = "",
    val type: String? = null,
    val serverVersion: String? = null,
    val openSubsonic: Boolean = false,
)

@Serializable
data class OpenSubsonicExtension(val name: String, val versions: List<Int> = emptyList())

@Serializable
data class ScanStatus(
    val scanning: Boolean = false,
    val count: Long? = null,
    val folderCount: Long? = null,
    val lastScan: String? = null,
)

@Serializable
data class User(
    val username: String,
    val email: String? = null,
    val scrobblingEnabled: Boolean = false,
    val adminRole: Boolean = false,
    val settingsRole: Boolean = false,
    val downloadRole: Boolean = false,
    val streamRole: Boolean = false,
    val playlistRole: Boolean = false,
    val coverArtRole: Boolean = false,
    val shareRole: Boolean = false,
)

@Serializable
data class ReplayGain(
    val trackGain: Double? = null,
    val albumGain: Double? = null,
    val trackPeak: Double? = null,
    val albumPeak: Double? = null,
    val baseGain: Double? = null,
)

/** A song (Subsonic "Child"). */
@Serializable
data class Song(
    val id: String,
    val parent: String? = null,
    val title: String = "",
    val album: String? = null,
    val albumId: String? = null,
    val artist: String? = null,
    val artistId: String? = null,
    val track: Int? = null,
    val discNumber: Int? = null,
    val year: Int? = null,
    val genre: String? = null,
    val coverArt: String? = null,
    val size: Long? = null,
    val contentType: String? = null,
    val suffix: String? = null,
    val duration: Int? = null,
    val bitRate: Int? = null,
    val bitDepth: Int? = null,
    val samplingRate: Int? = null,
    val channelCount: Int? = null,
    val starred: String? = null,
    val userRating: Int? = null,
    val playCount: Long? = null,
    val replayGain: ReplayGain? = null,
)

@Serializable
data class Album(
    val id: String,
    val name: String = "",
    val artist: String? = null,
    val artistId: String? = null,
    val coverArt: String? = null,
    val songCount: Int = 0,
    val duration: Int = 0,
    val playCount: Long? = null,
    val created: String? = null,
    val starred: String? = null,
    val year: Int? = null,
    val genre: String? = null,
    val played: String? = null,
    val userRating: Int? = null,
    val song: List<Song> = emptyList(),
)

@Serializable
data class Artist(
    val id: String,
    val name: String = "",
    val coverArt: String? = null,
    val artistImageUrl: String? = null,
    val albumCount: Int = 0,
    val starred: String? = null,
    val album: List<Album> = emptyList(),
)

/** `getArtistInfo2`. Image URLs may point anywhere: the app fetches only what is on the server. */
@Serializable
data class ArtistInfo(
    val biography: String? = null,
    val smallImageUrl: String? = null,
    val mediumImageUrl: String? = null,
    val largeImageUrl: String? = null,
)

@Serializable
data class ArtistIndex(val name: String = "", val artist: List<Artist> = emptyList())

@Serializable
data class ArtistsIndex(val ignoredArticles: String = "", val index: List<ArtistIndex> = emptyList()) {
    val all: List<Artist> get() = index.flatMap { it.artist }
}

@Serializable
data class SearchResult(
    val artist: List<Artist> = emptyList(),
    val album: List<Album> = emptyList(),
    val song: List<Song> = emptyList(),
)

@Serializable
data class Playlist(
    val id: String,
    val name: String = "",
    val comment: String? = null,
    val owner: String? = null,
    val public: Boolean = false,
    val songCount: Int = 0,
    val duration: Int = 0,
    val created: String? = null,
    val changed: String? = null,
    val coverArt: String? = null,
    val readonly: Boolean? = null,
    val entry: List<Song> = emptyList(),
)

@Serializable
data class Genre(val value: String = "", val songCount: Int = 0, val albumCount: Int = 0)

/** `getStarred2`. */
@Serializable
data class Starred(
    val artist: List<Artist> = emptyList(),
    val album: List<Album> = emptyList(),
    val song: List<Song> = emptyList(),
)

/** The `type` of getAlbumList2. byYear and byGenre take their extra parameters in the call. */
enum class AlbumListType(val wire: String) {
    Newest("newest"),
    Recent("recent"),
    Frequent("frequent"),
    Random("random"),
    Highest("highest"),
    Starred("starred"),
    AlphabeticalByName("alphabeticalByName"),
    AlphabeticalByArtist("alphabeticalByArtist"),
    ByYear("byYear"),
    ByGenre("byGenre"),
}
