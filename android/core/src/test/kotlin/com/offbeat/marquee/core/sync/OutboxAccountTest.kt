package com.offbeat.marquee.core.sync

import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The outbox belongs to one account: nothing queued signed out, nothing of one account sent for another. */
class OutboxAccountTest {
    private class MemoryFile(var text: String? = null) : TextFile {
        override fun read() = text
        override fun write(text: String) { this.text = text }
    }

    private class Recorder : OutboxSender {
        val scrobbled = mutableListOf<String>()
        val starred = mutableListOf<String>()
        override suspend fun scrobble(ids: List<String>, timesMs: List<Long>) { scrobbled += ids }
        override suspend fun star(kind: String, id: String, starred: Boolean) { this.starred += id }
    }

    private val alice = Outbox.accountOf("https://music.example.com/", "alice")
    private val bob = Outbox.accountOf("https://other.example.org", "bob")

    @Test
    fun accountKeyIgnoresCaseAndTrailingSlashAndHidesTheAddress() {
        assertEquals(alice, Outbox.accountOf("HTTPS://Music.Example.com", "alice"))
        assertNotEquals(alice, Outbox.accountOf("https://music.example.com", "alicia"))
        assertFalse("example" in alice)
    }

    @Test
    fun nothingIsQueuedWhileSignedOut() {
        val box = Outbox(MemoryFile(), clock = { 0 }, account = { null })
        assertFalse(box.setStar("song", "s1", true, 1))
        assertFalse(box.addScrobble("s1", 1))
        assertTrue(box.isEmpty)
    }

    @Test
    fun aWriteMadeForAnAccountThatIsGoneIsRefused() {
        var current: String? = bob
        val box = Outbox(MemoryFile(), clock = { 0 }, account = { current })
        // A heart tapped as alice, whose send failed after bob signed in.
        assertFalse(box.setStar("song", "s1", true, 1, forAccount = alice))
        assertTrue(box.isEmpty)
        current = null
        assertFalse(box.setStar("song", "s1", true, 1, forAccount = alice))
    }

    @Test
    fun anotherAccountsEntriesAreDroppedNotSent() = runTest {
        val file = MemoryFile()
        var current: String? = alice
        Outbox(file, clock = { 0 }, account = { current }).apply {
            assertTrue(addScrobble("a-song", 1))
            assertTrue(setStar("album", "a-album", true, 2))
        }
        // A sign-out whose cleanup never ran, then bob: alice's entries never reach bob's server.
        current = bob
        val box = Outbox(file, clock = { 0 }, account = { current })
        val sender = Recorder()
        assertEquals(FlushResult.Empty, box.flush(sender))
        assertTrue(sender.scrobbled.isEmpty() && sender.starred.isEmpty())
        assertTrue(box.isEmpty)
        // Bob's own go.
        assertTrue(box.addScrobble("b-song", 3))
        assertEquals(FlushResult.Done, box.flush(sender))
        assertEquals(listOf("b-song"), sender.scrobbled)
    }

    @Test
    fun queueingForANewAccountDropsTheOldOnes() {
        val file = MemoryFile()
        var current: String? = alice
        val box = Outbox(file, clock = { 0 }, account = { current })
        box.addScrobble("a-song", 1)
        current = bob
        box.addScrobble("b-song", 2)
        assertEquals(listOf("b-song"), box.snapshot().scrobbles.map { it.id })
        assertEquals(bob, box.snapshot().account)
    }

    @Test
    fun anOutboxFromBeforeAccountsIsTheCurrentAccounts() = runTest {
        val file = MemoryFile("""{"scrobbles":[{"id":"old","timeMs":5}]}""")
        val box = Outbox(file, clock = { 0 }, account = { alice })
        val sender = Recorder()
        assertEquals(FlushResult.Done, box.flush(sender))
        assertEquals(listOf("old"), sender.scrobbled)
    }

    @Test
    fun signedOutNothingIsFlushed() = runTest {
        val file = MemoryFile()
        var current: String? = alice
        val box = Outbox(file, clock = { 0 }, account = { current })
        box.addScrobble("a-song", 1)
        current = null
        val sender = Recorder()
        assertEquals(FlushResult.Empty, box.flush(sender))
        assertTrue(sender.scrobbled.isEmpty())
    }
}
