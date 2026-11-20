package com.offbeat.marquee.core.offline

import com.offbeat.marquee.core.library.SafeFile
import com.offbeat.marquee.core.subsonic.Song
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/** One damaged pins file must not cost every download. */
class PinsFileTest {
    @get:Rule
    val tmp = TemporaryFolder()

    private fun file() = File(tmp.root, "downloads/pins.json")
    private fun pins() = PinSet().with(Pin(PinKind.Album, "al", songs = listOf(Song("1", title = "1"), Song("2", title = "2")))).pins

    @Test
    fun noFileIsNoPinsAndTrusted() {
        val load = PinSet.load(file())
        assertTrue(load.trusted)
        assertTrue(load.pins.pins.isEmpty())
    }

    @Test
    fun aWrittenFileComesBack() {
        SafeFile.write(file(), pins().encode())
        val load = PinSet.load(file())
        assertTrue(load.trusted)
        assertEquals(setOf("1", "2"), load.pins.wanted())
        assertFalse(SafeFile.tmpOf(file()).exists())
    }

    @Test
    fun aDamagedFileIsKeptAsideAndNotTrusted() {
        file().parentFile!!.mkdirs()
        val damaged = pins().encode().take(20)
        file().writeText(damaged)
        val logged = mutableListOf<String>()
        val load = PinSet.load(file()) { logged += it }
        // Before: decode gave an empty set, the same as "no file", and reconcile deleted every song.
        assertFalse(load.trusted)
        assertTrue(load.pins.pins.isEmpty())
        assertEquals(damaged, SafeFile.badOf(file()).readText())
        assertFalse(file().exists())
        assertTrue(logged.any { "no download is deleted" in it })
    }

    @Test
    fun aWriteWhoseRenameFailedIsReadFromTheTempFile() {
        // The old fallback (delete, then a second rename that fails) left only pins.json.tmp.
        val t = SafeFile.tmpOf(file())
        t.parentFile!!.mkdirs()
        t.writeText(pins().encode())
        val load = PinSet.load(file())
        assertTrue(load.trusted)
        assertEquals(setOf("1", "2"), load.pins.wanted())
    }

    @Test
    fun theFileWinsOverALeftoverTempFile() {
        SafeFile.write(file(), pins().encode())
        SafeFile.tmpOf(file()).writeText("{half")
        assertEquals(setOf("1", "2"), PinSet.load(file()).pins.wanted())
    }

    @Test
    fun aWriteReplacesTheFileWhole() {
        SafeFile.write(file(), "one")
        SafeFile.write(file(), "two")
        assertEquals("two", SafeFile.read(file()))
        SafeFile.delete(file())
        assertEquals(null, SafeFile.read(file()))
    }
}
