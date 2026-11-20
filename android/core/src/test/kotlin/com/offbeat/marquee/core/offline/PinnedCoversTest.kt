package com.offbeat.marquee.core.offline

import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.core.subsonic.Song
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PinnedCoversTest {
    private fun album(id: String, cover: String, vararg songCovers: String?) = Pin(
        PinKind.Album, id, coverArt = cover,
        songs = songCovers.mapIndexed { i, c -> Song("$id-$i", coverArt = c) },
    )

    private fun playlist(id: String, cover: String?, vararg songCovers: String?) = Pin(
        PinKind.Playlist, id, coverArt = cover,
        songs = songCovers.mapIndexed { i, c -> Song("$id-$i", coverArt = c) },
    )

    @Test
    fun theAlbumsCoverAtEverySizeAndItsSongsAtTwo() {
        val w = PinnedCovers.wanted(PinSet(listOf(album("a", "al-a", "al-a", "mf-1", "mf-2", null, "mf-1"))))
        assertEquals(PinnedCovers.MAIN_SIZES, w.filter { it.coverId == "al-a" }.map { it.size }.toSet())
        assertEquals(setOf(CoverSize.THUMB, CoverSize.HEADER), w.filter { it.coverId == "mf-1" }.map { it.size }.toSet())
        // The album's own cover once, each song cover once (no duplicates, nulls skipped).
        assertEquals(4 + 2 + 2, w.size)
        // The pin's own cover first: it is what the grid and the page show.
        assertEquals(CoverFile("al-a", CoverSize.THUMB), w.first())
    }

    @Test
    fun aCoverSharedByTwoPinsIsKeptUntilTheLastGoes() {
        val a = album("a", "al-a", "mf-1")
        val p = playlist("p", null, "mf-1", "mf-9")
        val both = PinSet(listOf(a, p))
        val stored = PinnedCovers.wanted(both).toSet()
        // Removing the album: its own cover goes, the shared song cover stays for the playlist.
        val afterAlbum = PinnedCovers.plan(PinnedCovers.wanted(both.without(a.key).pins), stored)
        assertTrue(afterAlbum.fetch.isEmpty())
        assertEquals(PinnedCovers.MAIN_SIZES.map { CoverFile("al-a", it) }.toSet(), afterAlbum.delete)
        // Removing everything: everything goes.
        val none = PinnedCovers.plan(PinnedCovers.wanted(PinSet()), stored)
        assertEquals(stored, none.delete)
        assertTrue(none.fetch.isEmpty())
    }

    @Test
    fun planFetchesOnlyWhatIsMissing() {
        val wanted = PinnedCovers.wanted(PinSet(listOf(album("a", "al-a"))))
        val stored = setOf(CoverFile("al-a", CoverSize.THUMB), CoverFile("old", CoverSize.GRID))
        val plan = PinnedCovers.plan(wanted, stored)
        assertEquals(listOf(CoverSize.GRID, CoverSize.HEADER, CoverSize.FULL), plan.fetch.map { it.size })
        assertEquals(setOf(CoverFile("old", CoverSize.GRID)), plan.delete)
    }

    @Test
    fun longPlaylistsAreCapped() {
        val covers = (1..500).map { "mf-$it" }.toTypedArray()
        val w = PinnedCovers.wanted(PinSet(listOf(playlist("p", "pl-p", *covers))))
        assertEquals(PinnedCovers.MAIN_SIZES.size + PinnedCovers.MAX_SONG_COVERS_PER_PIN * PinnedCovers.SONG_SIZES.size, w.size)
    }

    @Test
    fun pickTheStoredSizeThatAnswers() {
        val all = PinnedCovers.MAIN_SIZES
        val song = PinnedCovers.SONG_SIZES
        assertEquals(CoverSize.GRID, PinnedCovers.pick(CoverSize.GRID, all, offline = false))
        // Not stored at that size: a larger one scales down, online or not.
        assertEquals(CoverSize.HEADER, PinnedCovers.pick(CoverSize.GRID, song, offline = false))
        // Nothing larger: only offline does it scale the largest up; online the server is sharper.
        assertNull(PinnedCovers.pick(CoverSize.FULL, song, offline = false))
        assertEquals(CoverSize.HEADER, PinnedCovers.pick(CoverSize.FULL, song, offline = true))
        assertNull(PinnedCovers.pick(CoverSize.GRID, emptySet(), offline = true))
    }

    @Test
    fun fileNamesRoundTrip() {
        for (id in listOf("al-3f2a_67c1d0e5", "mf-x", "odd/id with spaces%@", "ünï")) {
            val f = CoverFile(id, CoverSize.HEADER)
            val name = PinnedCovers.fileName(f)
            assertTrue(name, name.all { it.isLetterOrDigit() && it.code < 128 || it in "._-%@" })
            assertEquals(f, PinnedCovers.parse(name))
        }
        assertNull(PinnedCovers.parse("nonsense.tmp"))
        assertNull(PinnedCovers.parse("x@0.img"))
        assertNull(PinnedCovers.parse("@300.img"))
    }
}
