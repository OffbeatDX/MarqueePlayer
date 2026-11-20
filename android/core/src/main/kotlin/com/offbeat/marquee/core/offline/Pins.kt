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
    /** A playlist answer that would have dropped most of its songs, not followed yet ([PlaylistShrink]). */
    val shrinkSeen: String? = null,
    val shrinkSeenAtMs: Long = 0,
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
data class PinChange(
    val pins: PinSet,
    val fetch: Set<String>,
    val release: Set<String>,
    /** A playlist's suspect shrink was held back ([PlaylistShrink]): its songs did not change. */
    val held: Boolean = false,
)

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
        val ids = songs.map { it.id }
        if (PlaylistShrink.isSuspect(old.songIds.distinct().size, ids.distinct().size)) {
            val seen = PlaylistShrink.fingerprint(ids)
            if (!PlaylistShrink.isConfirmed(old.shrinkSeen, old.shrinkSeenAtMs, seen, nowMs)) {
                val since = if (old.shrinkSeen == seen) old.shrinkSeenAtMs else nowMs
                return with(old.copy(shrinkSeen = seen, shrinkSeenAtMs = since)).copy(held = true)
            }
        }
        return with(old.copy(songs = songs, changed = changed, syncedAtMs = nowMs, shrinkSeen = null, shrinkSeenAtMs = 0))
    }

    private fun changeTo(next: PinSet): PinChange {
        val before = wanted()
        val after = next.wanted()
        return PinChange(next, after - before, before - after)
    }

    fun encode(): String = json.encodeToString(serializer(), this)

    companion object {
        private val json = Json { ignoreUnknownKeys = true; encodeDefaults = false }

        fun decode(text: String?): PinSet = text?.let(::decodeOrNull) ?: PinSet()

        /** The pins in [text], or null when it is not a pins file (damaged, cut short). */
        fun decodeOrNull(text: String): PinSet? = runCatching { json.decodeFromString(serializer(), text) }.getOrNull()

        /**
         * Reads the pins file. No file is no pins. A file that is there but can't be read or decoded
         * is kept aside as `pins.json.bad` and the pins come back empty and *untrusted*: nothing may be
         * deleted for not being pinned in that run, or one damaged file would delete every download.
         */
        fun load(file: java.io.File, log: (String) -> Unit = {}): PinsLoad {
            val text = try {
                com.offbeat.marquee.core.library.SafeFile.read(file)
            } catch (e: java.io.IOException) {
                log("pins: ${file.name} can't be read (${e.javaClass.simpleName})")
                setAside(file, log)
                return PinsLoad(PinSet(), trusted = false)
            } ?: return PinsLoad(PinSet(), trusted = true)
            val pins = decodeOrNull(text)
            if (pins == null) {
                log("pins: ${file.name} is damaged (${text.length} chars)")
                setAside(file, log)
                return PinsLoad(PinSet(), trusted = false)
            }
            return PinsLoad(pins, trusted = true)
        }

        private fun setAside(file: java.io.File, log: (String) -> Unit) {
            val bad = com.offbeat.marquee.core.library.SafeFile.setAside(file)
            log("pins: ${if (bad != null) "kept aside as ${bad.name}" else "could not be kept aside"}; no download is deleted this run")
        }
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

/** The pins as read at start; [trusted] is false when the file was there but damaged ([PinSet.load]). */
data class PinsLoad(val pins: PinSet, val trusted: Boolean)

/**
 * A pinned playlist the server suddenly lists as empty, or with under a quarter of its songs, may be
 * a server mid-scan or a passing fault rather than the owner's edit: following it at once would delete
 * the downloads of every song gone. Such an answer is held ([isSuspect]) and followed only when a later
 * one, at least [CONFIRM_AFTER_MS] on, says the same ([isConfirmed]); a smaller change is followed at once.
 */
object PlaylistShrink {
    const val CONFIRM_AFTER_MS = 10 * 60_000L

    fun isSuspect(before: Int, after: Int): Boolean =
        before > 0 && (after == 0 || (before >= 8 && after * 4 < before))

    fun fingerprint(ids: List<String>): String = "${ids.size}:${ids.joinToString(",").hashCode()}"

    fun isConfirmed(seen: String?, seenAtMs: Long, now: String, nowMs: Long): Boolean =
        seen == now && nowMs - seenAtMs >= CONFIRM_AFTER_MS
}
