package com.offbeat.marquee.core.auto

import com.offbeat.marquee.core.lyrics.Fold
import com.offbeat.marquee.core.offline.Pin
import com.offbeat.marquee.core.offline.PinKind
import com.offbeat.marquee.core.subsonic.Song

/**
 * Search with no network: only what is downloaded. Every word of the query must appear (folded:
 * case, diacritics, full width) in the item's name, artist or album.
 */
object OfflineSearch {
    data class Result(val collections: List<Pin>, val songs: List<Song>)

    fun matches(query: String, vararg fields: String?): Boolean {
        val words = Fold.fold(query).split(' ').filter { it.isNotEmpty() }
        if (words.isEmpty()) return false
        val text = fields.filterNotNull().joinToString(" ") { Fold.fold(it) }
        return words.all { it in text }
    }

    /** Pinned albums and playlists by name or artist, then songs (each once) by title, artist or album. */
    fun search(pins: List<Pin>, query: String, maxSongs: Int = 50): Result {
        val collections = pins.filter { it.kind != PinKind.Song && matches(query, it.name, it.artist) }
        val seen = HashSet<String>()
        val songs = pins.asSequence()
            .flatMap { it.songs.asSequence() }
            .filter { seen.add(it.id) && matches(query, it.title, it.artist, it.album) }
            .take(maxSongs)
            .toList()
        return Result(collections, songs)
    }
}
