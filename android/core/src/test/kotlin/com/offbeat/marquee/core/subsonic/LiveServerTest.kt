package com.offbeat.marquee.core.subsonic

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import java.io.File
import java.util.Properties

/**
 * Against a real server, read-only, when ~/.marquee-android/test-server.properties exists (url,
 * user, salt and token); skipped otherwise. Nothing from the file or the server's answers is printed: failures
 * name only the error type.
 */
class LiveServerTest {
    private lateinit var client: SubsonicClient

    @Before
    fun setUp() {
        val file = File(System.getProperty("user.home"), ".marquee-android/test-server.properties")
        assumeTrue("no test server configured", file.isFile)
        val p = Properties().apply { file.inputStream().use { load(it) } }
        val creds = SubsonicCredentials(
            url = p.getProperty("url").orEmpty(),
            user = p.getProperty("user").orEmpty(),
            salt = p.getProperty("salt").orEmpty(),
            token = p.getProperty("token").orEmpty(),
        )
        // Always read-only here, whatever the file says.
        client = SubsonicClient(creds, readOnly = true)
    }

    private fun <T> live(block: suspend () -> T): T = runBlocking {
        try {
            block()
        } catch (e: SubsonicError) {
            fail("Live call failed: ${e::class.simpleName}")
            throw e
        }
    }

    @Test
    fun ping() {
        val info = live { client.ping() }
        assertEquals("ok", info.status)
        assertTrue(info.openSubsonic)
        assertTrue(!info.serverVersion.isNullOrBlank())
    }

    @Test
    fun newestAlbums() {
        val albums = live { client.getAlbumList2(AlbumListType.Newest, size = 5) }
        assertTrue(albums.size in 1..5)
        assertTrue(albums.all { it.id.isNotBlank() && it.name.isNotBlank() })
    }

    @Test
    fun search() {
        // Search for a word from a real album name, so the test does not depend on the library.
        val album = live { client.getAlbumList2(AlbumListType.Newest, size = 1) }.first()
        val word = album.name.split(' ').maxByOrNull { it.length }!!
        val result = live { client.search3(word, artistCount = 5, albumCount = 5, songCount = 5) }
        assertTrue(result.album.isNotEmpty() || result.song.isNotEmpty() || result.artist.isNotEmpty())
    }

    @Test
    fun starIsRefusedLocally() {
        runBlocking {
            try {
                client.star(id = "never-sent")
                fail("star was not refused")
            } catch (e: SubsonicError.RefusedReadOnly) {
                assertEquals("star", e.endpoint)
            }
        }
    }
}
