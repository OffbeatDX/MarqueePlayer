package com.offbeat.marquee.core.settings

import com.offbeat.marquee.core.art.Accent
import com.offbeat.marquee.core.library.ShelfId
import com.offbeat.marquee.core.library.Shelves
import com.offbeat.marquee.core.playback.CacheKeys
import com.offbeat.marquee.core.playback.NetworkKind
import com.offbeat.marquee.core.playback.QualitySettings
import com.offbeat.marquee.core.playback.SourcePicker
import com.offbeat.marquee.core.playback.StreamQuality
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SettingsModelsTest {

    // ---- Shelves ------------------------------------------------------------------------

    @Test
    fun defaultLayoutShowsEveryShelfInTheDesktopOrder() {
        assertEquals(Shelves.DEFAULT, ShelfLayout.DEFAULT.visible)
        assertTrue(ShelfLayout.DEFAULT.isDefault)
    }

    @Test
    fun movingAShelfReordersAndHidingRemovesItFromWhatHomeShows() {
        val moved = ShelfLayout.DEFAULT.move(ShelfId.Favourites, 0)
        assertEquals(ShelfId.Favourites, moved.order.first())
        assertEquals(ShelfId.RecentlyAdded, moved.order[1])
        val hidden = moved.setShown(ShelfId.Random, false)
        assertFalse(ShelfId.Random in hidden.visible)
        assertEquals(6, hidden.order.size)
        assertEquals(listOf(ShelfId.Favourites, ShelfId.RecentlyAdded, ShelfId.RecentlyPlayed, ShelfId.MostPlayed, ShelfId.RecentlyReleased), hidden.visible)
        // Out of range clamps; moving to the same place changes nothing.
        assertEquals(ShelfId.RecentlyAdded, ShelfLayout.DEFAULT.move(ShelfId.RecentlyAdded, 99).order.last())
        assertEquals(ShelfLayout.DEFAULT, ShelfLayout.DEFAULT.move(ShelfId.Random, 2))
    }

    @Test
    fun theLastVisibleShelfCannotBeHidden() {
        var l = ShelfLayout.DEFAULT
        for (id in Shelves.DEFAULT.drop(1)) l = l.setShown(id, false)
        assertEquals(listOf(ShelfId.RecentlyAdded), l.visible)
        assertFalse(l.canHide(ShelfId.RecentlyAdded))
        assertEquals(l, l.setShown(ShelfId.RecentlyAdded, false))
        assertTrue(l.canHide(ShelfId.Random)) // already hidden: can be shown again
    }

    @Test
    fun layoutSurvivesEncodingAndBadTextGivesTheDefault() {
        val l = ShelfLayout.DEFAULT.move(ShelfId.MostPlayed, 1).setShown(ShelfId.Random, false)
        assertEquals(l, ShelfLayout.decode(l.encode()))
        assertEquals(ShelfLayout.DEFAULT, ShelfLayout.decode(null))
        assertEquals(ShelfLayout.DEFAULT, ShelfLayout.decode(""))
        assertEquals(ShelfLayout.DEFAULT, ShelfLayout.decode("nonsense,,!also"))
        // Everything hidden (hand-edited prefs) shows everything rather than nothing.
        val allHidden = Shelves.DEFAULT.joinToString(",") { "!" + it.key }
        assertEquals(Shelves.DEFAULT, ShelfLayout.decode(allHidden).visible)
    }

    @Test
    fun aShelfMissingFromStoredTextGoesInAtItsDefaultPlace() {
        // Stored before Favourites existed (say): it comes back after Recently released, shown.
        val stored = "mostplayed,recentlyadded,recentlyplayed,random,recentlyreleased"
        val l = ShelfLayout.decode(stored)
        assertEquals(6, l.order.size)
        assertEquals(ShelfId.Favourites, l.order[l.order.indexOf(ShelfId.RecentlyReleased) + 1])
        assertEquals(ShelfId.MostPlayed, l.order.first())
        // Duplicates are ignored.
        assertEquals(ShelfLayout.DEFAULT, ShelfLayout.decode(ShelfLayout.DEFAULT.encode() + ",random"))
    }

    // ---- Cache cap ----------------------------------------------------------------------

    @Test
    fun loweringTheCapEvictsTheOldestUntilItFits() {
        val mb = 1024L * 1024
        val sizes = listOf(300 * mb, 500 * mb, 400 * mb, 900 * mb) // oldest first, 2100 MB in all
        val used = sizes.sum()
        assertEquals(0, CacheCap.evictions(sizes, used, CacheCap.bytes(4)))
        assertEquals(0, CacheCap.evictions(sizes, used, CacheCap.bytes(8)))
        // 2 GB = 2048 MB: the oldest 300 MB is enough.
        assertEquals(1, CacheCap.evictions(sizes, used, CacheCap.bytes(2)))
        // 1 GB = 1024 MB: 2100 - 300 - 500 = 1300, - 400 = 900.
        assertEquals(3, CacheCap.evictions(sizes, used, CacheCap.bytes(1)))
        // Room for what is about to be written, too.
        assertEquals(2, CacheCap.evictions(sizes, used, CacheCap.bytes(2), incoming = 400 * mb))
        // Never more than there is.
        assertEquals(4, CacheCap.evictions(sizes, used, 0))
    }

    @Test
    fun storedCapsMapToTheChoices() {
        assertEquals(2, CacheCap.gbOf(CacheCap.bytes(2)))
        assertEquals(8, CacheCap.gbOf(CacheCap.bytes(8)))
        assertEquals(4, CacheCap.gbOf(CacheCap.bytes(5)))
        assertEquals(1, CacheCap.gbOf(100))
        assertEquals(CacheCap.DEFAULT_GB, CacheCap.gbOf(-1))
    }

    // ---- Prefetch and quality -------------------------------------------------------------

    @Test
    fun prefetchCountIsClampedToZeroToFive() {
        assertEquals(0, PrefetchAhead.clamp(-3))
        assertEquals(5, PrefetchAhead.clamp(10))
        assertEquals(2, PrefetchAhead.DEFAULT)
    }

    @Test
    fun theOfferedQualitiesAreTheFiveAndAllRoundTrip() {
        assertEquals(listOf("Original", "Opus 320", "Opus 192", "Opus 128", "Opus 96"), QualityChoices.OFFERED.map(QualityChoices::label))
        for (q in QualityChoices.OFFERED) {
            assertEquals(q, StreamQuality.fromId(q.id, StreamQuality.ORIGINAL))
            assertTrue(q in StreamQuality.CHOICES) // stored copies in it are found
        }
        assertEquals(StreamQuality.opus(96), QualityChoices.nearestOffered(StreamQuality.opus(64)))
        assertEquals(StreamQuality.opus(128), QualityChoices.nearestOffered(StreamQuality.opus(128)))
    }

    @Test
    fun aChangedQualitySettingDecidesTheNextStream() {
        val nothingStored = { _: String -> false }
        val before = QualitySettings()
        assertEquals(StreamQuality.opus(128), SourcePicker.pick("s", NetworkKind.Metered, before, isDownloaded = nothingStored, isFullyCached = nothingStored))
        val after = before.copy(metered = StreamQuality.opus(320), unmetered = StreamQuality.opus(192))
        assertEquals(StreamQuality.opus(320), SourcePicker.pick("s", NetworkKind.Metered, after, isDownloaded = nothingStored, isFullyCached = nothingStored))
        assertEquals(StreamQuality.opus(192), SourcePicker.pick("s", NetworkKind.Unmetered, after, isDownloaded = nothingStored, isFullyCached = nothingStored))
        // A whole original copy in the cache still wins.
        val originalCached = { key: String -> key == CacheKeys.song("s", StreamQuality.ORIGINAL) }
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Metered, after, isDownloaded = nothingStored, isFullyCached = originalCached))
    }

    // ---- Accent -------------------------------------------------------------------------

    @Test
    fun fixedAccentIgnoresTheCoverAndEverySwatchIsUsable() {
        assertEquals(0x123456, AccentChoice.FromCover.effective(0x123456))
        assertEquals(AccentChoice.SWATCHES[3], AccentChoice.Fixed(AccentChoice.SWATCHES[3]).effective(0x123456))
        assertEquals(Accent.FALLBACK, AccentChoice.SWATCHES.first())
        assertEquals(8, AccentChoice.SWATCHES.size)
        assertEquals(AccentChoice.SWATCHES.size, AccentChoice.NAMES.size)
        assertEquals(8, AccentChoice.SWATCHES.toSet().size)
        for (s in AccentChoice.SWATCHES) assertEquals(s, Accent.usable(s).first)
        for (c in listOf(AccentChoice.FromCover, AccentChoice.Fixed(AccentChoice.SWATCHES[5]))) {
            assertEquals(c, AccentChoice.decode(c.encode()))
        }
        assertEquals(AccentChoice.FromCover, AccentChoice.decode("zz"))
        assertEquals(AccentChoice.FromCover, AccentChoice.decode(null))
    }
}
