package com.offbeat.marquee.core.library

/** What a long press can be on. */
enum class ItemKind { Album, Song, Artist, Playlist }

/** The long-press sheet's actions, in the order the sheet lists them. */
enum class ItemAction(val label: String) {
    PlayNext("Play next"),
    AddToQueue("Add to queue"),
    Shuffle("Shuffle"),
    GoToAlbum("Go to album"),
    GoToArtist("Go to artist"),
    Favourite("Favourite"),
    AddToPlaylist("Add to playlist"),
    Download("Download"),
    RemoveFromQueue("Remove from queue"),
}

/** Where the long press happened, for what the sheet leaves out. */
data class ActionPlace(
    /** A row of Now Playing's queue (the song is already queued). */
    val inQueue: Boolean = false,
    /** The queue's current song: it can't be moved or removed. */
    val isCurrent: Boolean = false,
    /** On this album's or artist's own page, so "Go to" would go nowhere. */
    val onOwnPage: Boolean = false,
)

object ItemActions {
    /**
     * The actions for an item. Download (or remove the download) for albums, playlists and single
     * songs. Favourite exists for songs, albums and artists (Subsonic has
     * none for playlists); Add to playlist for songs and albums.
     */
    fun applicable(
        kind: ItemKind,
        hasAlbum: Boolean = false,
        hasArtist: Boolean = false,
        place: ActionPlace = ActionPlace(),
    ): List<ItemAction> = buildList {
        if (place.inQueue) {
            // Already queued: Play next moves it up; adding it again is left to the other screens.
            if (!place.isCurrent) add(ItemAction.PlayNext)
        } else {
            add(ItemAction.PlayNext)
            add(ItemAction.AddToQueue)
        }
        if (kind != ItemKind.Song) add(ItemAction.Shuffle)
        val goAlbum = when (kind) {
            ItemKind.Song -> hasAlbum
            ItemKind.Album -> hasAlbum && !place.onOwnPage
            else -> false
        }
        if (goAlbum) add(ItemAction.GoToAlbum)
        val goArtist = when (kind) {
            ItemKind.Song, ItemKind.Album -> hasArtist
            ItemKind.Artist -> hasArtist && !place.onOwnPage
            ItemKind.Playlist -> false
        }
        if (goArtist) add(ItemAction.GoToArtist)
        if (kind != ItemKind.Playlist) add(ItemAction.Favourite)
        if (kind == ItemKind.Song || kind == ItemKind.Album) add(ItemAction.AddToPlaylist)
        if (kind != ItemKind.Artist) add(ItemAction.Download)
        if (place.inQueue && !place.isCurrent) add(ItemAction.RemoveFromQueue)
    }

    /** The endpoint an action writes to (refused in read-only builds); null when it only plays or navigates. */
    fun endpointOf(action: ItemAction, favourite: Boolean): String? = when (action) {
        ItemAction.Favourite -> if (favourite) "unstar" else "star"
        ItemAction.AddToPlaylist -> "updatePlaylist"
        else -> null
    }
}

/**
 * Values shown ahead of the server (favourites): set at once, then kept or rolled back
 * when the write answers. A rollback undoes only its own change, never a newer one made meanwhile.
 */
class Overrides<V : Any> {
    private val values = HashMap<String, V>()
    private val generation = HashMap<String, Long>()
    private var next = 0L

    /** A change in flight: what to roll back to, and which change it was. */
    data class Ticket<V>(val id: String, val previous: V?, val generation: Long)

    @Synchronized
    fun set(id: String, value: V): Ticket<V> {
        val t = Ticket(id, values[id], ++next)
        values[id] = value
        generation[id] = t.generation
        return t
    }

    /** Undoes [ticket]'s change; false when a newer change has replaced it (that one stays). */
    @Synchronized
    fun rollback(ticket: Ticket<V>): Boolean {
        if (generation[ticket.id] != ticket.generation) return false
        val previous = ticket.previous
        if (previous != null) values[ticket.id] = previous else values.remove(ticket.id)
        generation.remove(ticket.id)
        return true
    }

    /**
     * Adds the [loaded] values (kept from a previous run) for ids that have none yet: a change made
     * here before they arrived is newer and stays. True when anything was added.
     */
    @Synchronized
    fun fillMissing(loaded: Map<String, V>): Boolean {
        var added = false
        for ((id, v) in loaded) {
            if (id !in values) {
                values[id] = v
                added = true
            }
        }
        return added
    }

    @Synchronized
    operator fun get(id: String): V? = values[id]

    @Synchronized
    fun snapshot(): Map<String, V> = HashMap(values)

    @Synchronized
    fun clear() {
        values.clear()
        generation.clear()
    }
}
