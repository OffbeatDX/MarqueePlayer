package com.offbeat.marquee.core.offline

import com.offbeat.marquee.core.subsonic.Song
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/** What can be pinned for offline: an album, a playlist (which follows the server) or one song. */
@Serializable
enum class PinKind { Album, Playlist, Song }

/**
 * One pinned item and the songs it pins, with enough to show it offline (name, artist, cover,
 * the songs themselves). [changed] is the playlist's `changed` stamp when it was last synced.
 */
@Serializable
data class Pin(
    val kind: PinKind,
    val id: String,
    val name: String = "",
    val artist: String? = null,
    val artistId: String? = null,
    val coverArt: String? = null,
    val year: Int? = null,
    val songs: List<Song> = emptyList(),
    val pinnedAtMs: Long = 0,
    val syncedAtMs: Long = 0,
    val changed: String? = null,
) {
    val key: String get() = keyOf(kind, id)
    val songIds: List<String> get() = songs.map { it.id }

    companion object {
        fun keyOf(kind: PinKind, id: String): String = "${kind.name.lowercase()}:$id"
    }
}

/**
 * What a pin change means for the files: [fetch] are songs now wanted that were not before, [release]
 * songs nothing pins any more (their files can go).
 */
data class PinChange(val pins: PinSet, val fetch: Set<String>, val release: Set<String>)

/**
 * Every pin. A song shared by an album and a playlist (or pinned twice) is stored once: it is
 * wanted while anything pins it ([refCount] > 0) and released only when the last pin goes.
 */
@Serializable
data class PinSet(val pins: List<Pin> = emptyList()) {
    /** Every song some pin wants, in pin order then track order (the order downloads go in). */
    fun wanted(): LinkedHashSet<String> = pins.flatMapTo(LinkedHashSet()) { it.songIds }

    fun refCount(songId: String): Int = pins.count { p -> p.songs.any { it.id == songId } }

    operator fun get(key: String): Pin? = pins.firstOrNull { it.key == key }

    fun isPinned(kind: PinKind, id: String): Boolean = get(Pin.keyOf(kind, id)) != null

    /** Adds [pin], or replaces the pin with the same key (a playlist's new track list). */
    fun with(pin: Pin): PinChange {
        val i = pins.indexOfFirst { it.key == pin.key }
        val next = if (i < 0) pins + pin else pins.toMutableList().also { it[i] = pin }
        return changeTo(PinSet(next))
    }

    fun without(key: String): PinChange = changeTo(PinSet(pins.filterNot { it.key == key }))

    fun cleared(): PinChange = changeTo(PinSet())

    /** A pinned playlist's songs as the server now lists them; nothing changes when it is not pinned. */
    fun followPlaylist(id: String, songs: List<Song>, changed: String?, nowMs: Long): PinChange {
        val old = get(Pin.keyOf(PinKind.Playlist, id)) ?: return PinChange(this, emptySet(), emptySet())
        return with(old.copy(songs = songs, changed = changed, syncedAtMs = nowMs))
    }

    private fun changeTo(next: PinSet): PinChange {
        val before = wanted()
        val after = next.wanted()
        return PinChange(next, after - before, before - after)
    }

    fun encode(): String = json.encodeToString(serializer(), this)

    companion object {
        private val json = Json { ignoreUnknownKeys = true; encodeDefaults = false }

        fun decode(text: String?): PinSet =
            text?.let { runCatching { json.decodeFromString(serializer(), it) }.getOrNull() } ?: PinSet()
    }
}

/** A playlist's change between two syncs, by song id. */
data class ListDiff(val added: List<String>, val removed: List<String>) {
    val isEmpty: Boolean get() = added.isEmpty() && removed.isEmpty()

    companion object {
        fun of(old: List<String>, new: List<String>): ListDiff {
            val o = old.toSet()
            val n = new.toSet()
            return ListDiff(new.filter { it !in o }.distinct(), old.filter { it !in n }.distinct())
        }
    }
}

/**
 * What the download folder must do to match the pins: fetch what is wanted and not complete,
 * delete what is stored and not wanted (left over from an interrupted removal, say).
 */
data class Reconcile(val fetch: List<String>, val delete: Set<String>) {
    companion object {
        fun of(wanted: Collection<String>, stored: Set<String>, complete: Set<String>): Reconcile =
            Reconcile(wanted.filter { it !in complete }.distinct(), stored - wanted.toSet())
    }
}
