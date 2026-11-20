package com.offbeat.marquee.core.library

import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.AlbumListType
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class LibraryTest {
    @get:Rule
    val tmp = TemporaryFolder()

    @Test
    fun defaultShelvesAreTheDesktopsOrderWithTheHeroFirst() {
        assertEquals(ShelfId.RecentlyAdded, Shelves.DEFAULT.first())
        assertTrue(Shelves.DEFAULT.first().isHero)
        assertEquals(Shelves.DEFAULT.size, Shelves.DEFAULT.toSet().size)
        assertEquals(1, Shelves.DEFAULT.count { it.isRandom })
        Shelves.DEFAULT.forEach { assertEquals(it, ShelfId.fromKey(it.key)) }
    }

    @Test
    fun recentlyReleasedAsksByYearDescending() {
        val q = Shelves.query(ShelfId.RecentlyReleased, 20, 40, currentYear = 2026)
        assertEquals(AlbumListType.ByYear, q.type)
        assertTrue("fromYear > toYear sorts descending", q.fromYear!! > q.toYear!!)
        assertEquals(2027, q.fromYear)
        assertEquals(40, q.offset)
        val r = Shelves.query(ShelfId.RecentlyPlayed, 20, 0, 2026)
        assertEquals(AlbumListType.Recent, r.type)
        assertNull(r.fromYear)
    }

    @Test
    fun coverSizesSnapToBuckets() {
        assertEquals(CoverSize.THUMB, CoverSize.bucketFor(100))
        assertEquals(CoverSize.THUMB, CoverSize.bucketFor(160))
        assertEquals(CoverSize.GRID, CoverSize.bucketFor(300))
        assertEquals(CoverSize.HEADER, CoverSize.bucketFor(420))
        assertEquals(CoverSize.FULL, CoverSize.bucketFor(2000))
    }

    @Test
    fun diskStoreKeepsTheMostRecentlyUsed() {
        val dir = tmp.newFolder("store")
        val store = DiskStore(dir, maxEntries = 2)
        store.write("album-a", "A")
        store.write("album-b", "B")
        // File times are coarse on some file systems: set them explicitly.
        File(dir, "album-a.json").setLastModified(1_000_000)
        File(dir, "album-b.json").setLastModified(2_000_000)
        assertEquals("A", store.read("album-a")) // a is now the most recent
        store.write("album-c", "C")
        assertNull(store.read("album-b"))
        assertEquals("A", store.read("album-a"))
        assertEquals("C", store.read("album-c"))
        store.write("album-c", "C2")
        assertEquals("C2", store.read("album-c"))
        store.clear()
        assertNull(store.read("album-a"))
    }

    @Test
    fun diskStoreKeysAreSafeFileNames() {
        assertEquals("dc-abc_1", DiskStore.safeName("dc-abc:1"))
        assertEquals("a_b_c", DiskStore.safeName("a/b\\c"))
    }

    // ---- Sorts -------------------------------------------------------------------------------

    @Test
    fun sortsMapToTheServersListTypes() {
        assertEquals(AlbumListType.Newest, AlbumSort.RecentlyAdded.type)
        assertEquals(AlbumListType.AlphabeticalByName, AlbumSort.Name.type)
        assertEquals(AlbumListType.AlphabeticalByArtist, AlbumSort.Artist.type)
        assertEquals(AlbumListType.ByYear, AlbumSort.Year.type)
        assertEquals(AlbumListType.Frequent, AlbumSort.MostPlayed.type)
        assertEquals(AlbumListType.Random, AlbumSort.Random.type)
        AlbumSort.entries.forEach { assertEquals(it, AlbumSort.fromKey(it.key)) }
        assertEquals(AlbumSort.DEFAULT, AlbumSort.fromKey("nonsense"))
        assertEquals(AlbumSort.DEFAULT, AlbumSort.fromKey(null))
    }

    @Test
    fun yearSortAsksNewestFirstAndOthersPageByOffset() {
        val q = AlbumSort.Year.query(60, 120, currentYear = 2026)
        assertEquals(2027, q.fromYear)
        assertEquals(0, q.toYear)
        assertEquals(120, q.offset)
        assertNull(AlbumSort.Name.query(60, 0, 2026).fromYear)
        assertFalse(AlbumSort.Random.pagesByOffset)
        assertTrue(AlbumSort.entries.filter { it != AlbumSort.Random }.all { it.pagesByOffset })
        assertEquals(setOf(AlbumSort.Name, AlbumSort.Artist, AlbumSort.Year), AlbumSort.entries.filter { it.bubble }.toSet())
    }

    @Test
    fun lettersIgnoreArticlesAndFoldAccents() {
        val a = SortLetters.DEFAULT_ARTICLES
        assertEquals("W", SortLetters.letterOf("The Wall", a))
        assertEquals("M", SortLetters.letterOf("A Moon Shaped Pool", a))
        assertEquals("E", SortLetters.letterOf("Écoute", a))
        assertEquals("#", SortLetters.letterOf("1999", a))
        assertEquals("#", SortLetters.letterOf("", a))
        // A name that is only an article keeps it.
        assertEquals("T", SortLetters.letterOf("The", a))
        // "Theory" is not "The ory".
        assertEquals("T", SortLetters.letterOf("Theory of Everything", a))
        assertEquals(listOf("The", "A"), SortLetters.articles("The A"))
        assertEquals(SortLetters.DEFAULT_ARTICLES, SortLetters.articles(" "))
    }

    @Test
    fun bubbleTextFollowsTheSort() {
        val album = Album(id = "1", name = "The Dark Side", artist = "Pink Floyd", year = 1973)
        val a = SortLetters.DEFAULT_ARTICLES
        assertEquals("D", AlbumSort.Name.bubbleText(album, a))
        assertEquals("P", AlbumSort.Artist.bubbleText(album, a))
        assertEquals("1973", AlbumSort.Year.bubbleText(album, a))
        assertEquals("—", AlbumSort.Year.bubbleText(album.copy(year = 0), a))
        assertNull(AlbumSort.RecentlyAdded.bubbleText(album, a))
    }

    @Test
    fun localSortMatchesTheServersOrder() {
        val a = SortLetters.DEFAULT_ARTICLES
        val list = listOf(
            Album("1", name = "The Zoo", artist = "B", year = 2001, created = "2024-01-01", playCount = 3),
            Album("2", name = "Apple", artist = "The A Band", year = 1999, created = "2025-01-01", playCount = 9),
            Album("3", name = "Moon", artist = "C", year = 2010, created = "2023-01-01", playCount = 1),
        )
        assertEquals(listOf("2", "3", "1"), AlbumSort.Name.sortLocally(list, a).map { it.id })
        assertEquals(listOf("2", "1", "3"), AlbumSort.Artist.sortLocally(list, a).map { it.id })
        assertEquals(listOf("3", "1", "2"), AlbumSort.Year.sortLocally(list, a).map { it.id })
        assertEquals(listOf("2", "1", "3"), AlbumSort.RecentlyAdded.sortLocally(list, a).map { it.id })
        assertEquals(listOf("2", "1", "3"), AlbumSort.MostPlayed.sortLocally(list, a).map { it.id })
        assertEquals(AlbumSort.Random.sortLocally(list, a, 7), AlbumSort.Random.sortLocally(list, a, 7))
    }

    // ---- Paging ------------------------------------------------------------------------------

    @Test
    fun pagingMaths() {
        assertEquals(0, AlbumPaging.pageOf(59))
        assertEquals(1, AlbumPaging.pageOf(60))
        assertEquals(0, AlbumPaging.pageOf(-5))
        assertEquals(120, AlbumPaging.offsetOf(2))
        assertEquals(16, AlbumPaging.pageCount(960))
        assertEquals(17, AlbumPaging.pageCount(961))
        assertEquals(0, AlbumPaging.pageCount(0))
    }

    @Test
    fun wantedPagesCoverTheScreenAndTheMarginAroundIt() {
        // Top of the list: page 0 only (cells 0..11 plus 30 ahead).
        assertEquals(0..0, AlbumPaging.wantedPages(0, 11, 960))
        // Near a page end, the next page is fetched before it is reached.
        assertEquals(0..1, AlbumPaging.wantedPages(20, 35, 960))
        // A jump to the middle wants the pages there, not those between.
        assertEquals(7..8, AlbumPaging.wantedPages(470, 481, 960))
        // The end is clamped to the last page.
        assertEquals(15..15, AlbumPaging.wantedPages(950, 959, 960))
        assertTrue(AlbumPaging.wantedPages(0, 10, 0).isEmpty())
        assertTrue(AlbumPaging.wantedPages(0, -1, 960).isEmpty())
    }

    @Test
    fun fastScrollerFractionsAndIndices() {
        assertEquals(0, AlbumPaging.indexAt(0f, 960))
        assertEquals(959, AlbumPaging.indexAt(1f, 960))
        assertEquals(479, AlbumPaging.indexAt(0.5f, 960))
        assertEquals(959, AlbumPaging.indexAt(3f, 960))
        assertEquals(0, AlbumPaging.indexAt(0.5f, 0))
        assertEquals(0f, AlbumPaging.fractionOf(0, 12, 960), 0f)
        assertEquals(1f, AlbumPaging.fractionOf(948, 12, 960), 0f)
        assertEquals(0f, AlbumPaging.fractionOf(0, 12, 10), 0f)
    }

    @Test
    fun countSearchFindsTheExactSize() = runTest {
        for (size in listOf(0, 1, 59, 60, 960, 1023, 1024, 1025, 5000)) {
            var calls = 0
            val n = CountSearch.count(hint = null) { offset -> calls++; offset < size }
            assertEquals("size $size", size, n)
            assertTrue("size $size took $calls requests", calls <= 30)
        }
    }

    @Test
    fun countSearchConfirmsAGoodHintInTwoRequests() = runTest {
        var calls = 0
        assertEquals(960, CountSearch.count(hint = 960) { calls++; it < 960 })
        assertEquals(2, calls)
        // A stale hint (albums added or removed since) falls back to searching.
        assertEquals(975, CountSearch.count(hint = 960) { it < 975 })
        assertEquals(900, CountSearch.count(hint = 960) { it < 900 })
        assertEquals(0, CountSearch.count(hint = 960) { false })
    }

    // ---- Recent searches ---------------------------------------------------------------------

    @Test
    fun recentSearchesKeepTheLastTenNewestFirst() {
        var list = emptyList<String>()
        for (i in 1..12) list = RecentSearches.add(list, "q$i")
        assertEquals(10, list.size)
        assertEquals("q12", list.first())
        assertEquals("q3", list.last())
        list = RecentSearches.add(list, "  Q5 ")
        assertEquals("Q5", list.first())
        assertEquals(10, list.size)
        assertEquals(1, list.count { it.equals("q5", ignoreCase = true) })
        assertEquals(list, RecentSearches.add(list, "   "))
        assertEquals("daft punk", RecentSearches.add(emptyList(), "daft   punk").first())
        list = RecentSearches.remove(list, "Q5")
        assertFalse(list.any { it.equals("q5", ignoreCase = true) })
    }

    // ---- Long-press actions ------------------------------------------------------------------

    @Test
    fun songActions() {
        val a = ItemActions.applicable(ItemKind.Song, hasAlbum = true, hasArtist = true)
        assertEquals(
            listOf(
                ItemAction.PlayNext, ItemAction.AddToQueue, ItemAction.GoToAlbum, ItemAction.GoToArtist,
                ItemAction.Favourite, ItemAction.Rate, ItemAction.AddToPlaylist, ItemAction.Download,
            ),
            a,
        )
        val bare = ItemActions.applicable(ItemKind.Song)
        assertFalse(ItemAction.GoToAlbum in bare)
        assertFalse(ItemAction.GoToArtist in bare)
        assertFalse(ItemAction.Shuffle in bare)
        assertTrue(ItemAction.Download in bare) // a single song can be downloaded
    }

    @Test
    fun albumArtistAndPlaylistActions() {
        val album = ItemActions.applicable(ItemKind.Album, hasAlbum = true, hasArtist = true)
        assertTrue(listOf(ItemAction.Shuffle, ItemAction.GoToArtist, ItemAction.AddToPlaylist, ItemAction.Download).all { it in album })
        assertTrue(ItemAction.GoToAlbum in album)
        assertFalse(ItemAction.GoToAlbum in ItemActions.applicable(ItemKind.Album, true, true, ActionPlace(onOwnPage = true)))

        val artist = ItemActions.applicable(ItemKind.Artist, hasArtist = true)
        assertTrue(ItemAction.Shuffle in artist && ItemAction.Favourite in artist && ItemAction.Rate in artist)
        assertFalse(ItemAction.AddToPlaylist in artist || ItemAction.Download in artist || ItemAction.GoToAlbum in artist)

        val playlist = ItemActions.applicable(ItemKind.Playlist)
        assertEquals(listOf(ItemAction.PlayNext, ItemAction.AddToQueue, ItemAction.Shuffle, ItemAction.Download), playlist)
    }

    @Test
    fun queueRowActions() {
        val upcoming = ItemActions.applicable(ItemKind.Song, true, true, ActionPlace(inQueue = true))
        assertTrue(ItemAction.PlayNext in upcoming && ItemAction.RemoveFromQueue in upcoming)
        assertFalse(ItemAction.AddToQueue in upcoming)
        val current = ItemActions.applicable(ItemKind.Song, true, true, ActionPlace(inQueue = true, isCurrent = true))
        assertFalse(ItemAction.PlayNext in current || ItemAction.RemoveFromQueue in current)
        assertTrue(ItemAction.Favourite in current)
    }

    @Test
    fun writesAndRatings() {
        assertEquals("star", ItemActions.endpointOf(ItemAction.Favourite, favourite = false))
        assertEquals("unstar", ItemActions.endpointOf(ItemAction.Favourite, favourite = true))
        assertEquals("setRating", ItemActions.endpointOf(ItemAction.Rate, false))
        assertEquals("updatePlaylist", ItemActions.endpointOf(ItemAction.AddToPlaylist, false))
        assertNull(ItemActions.endpointOf(ItemAction.PlayNext, false))
        assertEquals(4, ItemActions.nextRating(2, 4))
        assertEquals(0, ItemActions.nextRating(4, 4))
    }

    @Test
    fun loadedOverridesNeverReplaceANewerChange() {
        val o = Overrides<Boolean>()
        val toggle = o.set("a", false) // made before the saved states finished loading
        assertTrue(o.fillMissing(mapOf("a" to true, "b" to true)))
        assertEquals(mapOf("a" to false, "b" to true), o.snapshot())
        assertFalse(o.fillMissing(mapOf("b" to false)))
        assertEquals(true, o["b"])
        // The toggle's own rollback still works after the load.
        assertTrue(o.rollback(toggle))
        assertEquals(mapOf("b" to true), o.snapshot())
    }

    @Test
    fun overridesRollBackOnlyTheirOwnChange() {
        val o = Overrides<Boolean>()
        val t1 = o.set("a", true)
        assertEquals(true, o["a"])
        assertTrue(o.rollback(t1))
        assertNull(o["a"])

        val first = o.set("b", true)
        val second = o.set("b", false)
        // The first write failing must not undo the newer one.
        assertFalse(o.rollback(first))
        assertEquals(false, o["b"])
        assertTrue(o.rollback(second))
        assertEquals(true, o["b"]) // back to what the first change had shown

        val ok = o.set("c", true)
        // A confirmed change simply stays.
        assertEquals(mapOf("b" to true, "c" to true), o.snapshot())
        o.clear()
        assertFalse(o.rollback(ok))
        assertTrue(o.snapshot().isEmpty())
    }
}
