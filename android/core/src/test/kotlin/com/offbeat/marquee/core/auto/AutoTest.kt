package com.offbeat.marquee.core.auto

import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.core.art.CoverUris
import com.offbeat.marquee.core.library.ShelfId
import com.offbeat.marquee.core.offline.Pin
import com.offbeat.marquee.core.offline.PinKind
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Android Auto: the browse tree's ids, offline search, and the cover URIs artwork goes out as. */
class AutoTest {
    // ---- Browse ids ----------------------------------------------------------------------

    @Test
    fun everyBrowseIdRoundTrips() {
        val ids = listOf(
            BrowseId.Root, BrowseId.Home, BrowseId.Playlists, BrowseId.Recent, BrowseId.Downloads,
            BrowseId.Shelf(ShelfId.RecentlyAdded), BrowseId.Shelf(ShelfId.Favourites),
            BrowseId.Album("al-1"), BrowseId.Playlist("pl 2/x"),
            BrowseId.Track(TrackList.Album("al/1"), 3, "song:1"),
            BrowseId.Track(TrackList.Playlist("p%"), 0, "s"),
            BrowseId.Track(TrackList.DownloadedSongs, 7, "s7"),
            BrowseId.Track(TrackList.Search("bach / goldberg ü"), 2, "s2"),
        )
        for (b in ids) {
            assertTrue(b.id, b.id.startsWith(BrowseId.PREFIX))
            assertEquals(b.id, b, BrowseId.parse(b.id))
        }
        // Each id is one media id: no two nodes share one.
        assertEquals(ids.size, ids.map { it.id }.toSet().size)
    }

    @Test
    fun songIdsAndJunkAreNotBrowseIds() {
        for (s in listOf(null, "", "abc123", "root", "@m/", "@m/nope", "@m/album/", "@m/shelf/unknown", "@m/track/album/a/x/s", "@m/track/album/a/-1/s", "@m/track/album/a/1", "@m/home/extra")) {
            assertNull(s, BrowseId.parse(s))
        }
    }

    @Test
    fun pagingSlicesWithoutOverflow() {
        val list = (0 until 25).toList()
        assertEquals((0 until 10).toList(), BrowsePaging.page(list, 0, 10))
        assertEquals((20 until 25).toList(), BrowsePaging.page(list, 2, 10))
        assertEquals(emptyList<Int>(), BrowsePaging.page(list, 3, 10))
        assertEquals(list, BrowsePaging.page(list, 0, Int.MAX_VALUE))
        assertEquals(emptyList<Int>(), BrowsePaging.page(list, 1, Int.MAX_VALUE))
        assertEquals(emptyList<Int>(), BrowsePaging.page(list, -1, 10))
    }

    @Test
    fun aPickedTrackStartsWhereItIsNow() {
        val ids = listOf("a", "b", "c", "b")
        assertEquals(3, BrowsePaging.startIndex(ids, 3, "b")) // the same song twice: the one picked
        assertEquals(1, BrowsePaging.startIndex(ids, 5, "b")) // the list got shorter: where it is now
        assertEquals(2, BrowsePaging.startIndex(ids, 0, "c"))
        assertEquals(0, BrowsePaging.startIndex(ids, 1, "gone"))
    }

    // ---- Offline search ------------------------------------------------------------------

    @Test
    fun offlineSearchLooksOnlyAtDownloads() {
        val s1 = Song("1", title = "Clair de lune", artist = "Debussy", album = "Suite bergamasque")
        val s2 = Song("2", title = "Arabesque", artist = "Debussy", album = "Suite bergamasque")
        val s3 = Song("3", title = "Gymnopédie No. 1", artist = "Satie", album = "Gymnopédies")
        val pins = listOf(
            Pin(PinKind.Album, "al", name = "Suite bergamasque", artist = "Claude Debussy", songs = listOf(s1, s2)),
            Pin(PinKind.Playlist, "pl", name = "Piano", songs = listOf(s3, s1)),
            Pin(PinKind.Song, "3", name = s3.title, songs = listOf(s3)),
        )
        val r = OfflineSearch.search(pins, "debussy")
        assertEquals(listOf("al"), r.collections.map { it.id })
        assertEquals(listOf("1", "2"), r.songs.map { it.id }) // each song once
        assertEquals(listOf("3"), OfflineSearch.search(pins, "GYMNOPEDIE satie").songs.map { it.id })
        assertEquals(listOf("pl"), OfflineSearch.search(pins, "piano").collections.map { it.id })
        assertTrue(OfflineSearch.search(pins, "mahler").songs.isEmpty())
        assertTrue(OfflineSearch.search(pins, "   ").songs.isEmpty())
        assertFalse(OfflineSearch.matches("lune debussy x", s1.title, s1.artist))
    }

    // ---- Cover URIs ------------------------------------------------------------------------

    private val authority = "com.offbeat.marquee.player.debug.covers"

    @Test
    fun coverUrisCarryNoCredentials() {
        val creds = SubsonicCredentials("https://music.example.org", "someone", "s4lt", "70k3n")
        val client = SubsonicClient(creds, readOnly = true)
        val id = "al-42_5f3a"
        val uri = CoverUris.uri(authority, id, CoverSize.GRID)
        assertEquals("content://$authority/cover/al-42_5f3a/300", uri)
        val server = client.coverArtUrl(id, CoverSize.GRID)
        // The server URL has them; the content URI none of them, nor the host or any query.
        assertTrue(server.contains("t=70k3n") && server.contains("s=s4lt"))
        for (secret in listOf("70k3n", "s4lt", "someone", "music.example.org", "?", "t=", "s=", "u=")) {
            assertFalse(secret, uri.contains(secret))
        }
    }

    @Test
    fun coverUrisParseOnlyWhatTheyMake() {
        val odd = "ar-1/x y?z"
        val uri = CoverUris.uri(authority, odd, CoverSize.THUMB)
        assertFalse(uri.contains(' ') || uri.substringAfter("/cover/").contains('?'))
        assertEquals(odd, CoverUris.coverIdOf(uri, authority))
        assertEquals(CoverUris.Request("al-1", 600), CoverUris.parse(listOf("cover", "al-1", "600")))
        assertNull(CoverUris.parse(listOf("cover", "al-1", "512"))) // not a bucket
        assertNull(CoverUris.parse(listOf("cover", "", "300")))
        assertNull(CoverUris.parse(listOf("cover", "al-1")))
        assertNull(CoverUris.parse(listOf("song", "al-1", "300")))
        assertNull(CoverUris.parse(listOf("cover", "al-1", "300", "x")))
        assertNull(CoverUris.coverIdOf("content://other.authority/cover/al-1/300", authority))
        assertNull(CoverUris.coverIdOf("marquee://cover/al-1?size=300", authority))
    }
}
