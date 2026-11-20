package com.offbeat.marquee.core.playback

import com.offbeat.marquee.core.playback.PlaybackRecovery.FailureKind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class StreamReplyTest {
    @Test
    fun jsonXmlAndTextAreNeverAudio() {
        assertTrue(StreamReply.isNotAudio("application/json"))
        assertTrue(StreamReply.isNotAudio("application/json; charset=utf-8"))
        assertTrue(StreamReply.isNotAudio("text/xml"))
        assertTrue(StreamReply.isNotAudio("application/xml"))
        assertTrue(StreamReply.isNotAudio("text/html; charset=UTF-8"))
        assertTrue(StreamReply.isNotAudio("TEXT/PLAIN"))
        assertFalse(StreamReply.isNotAudio("audio/flac"))
        assertFalse(StreamReply.isNotAudio("audio/ogg"))
        assertFalse(StreamReply.isNotAudio("application/octet-stream"))
        assertFalse(StreamReply.isNotAudio(null))
    }

    @Test
    fun readsTheSubsonicErrorCodeFromJsonAndXml() {
        val json = """{"subsonic-response":{"status":"failed","version":"1.16.1","type":"navidrome","serverVersion":"0.64.2","openSubsonic":true,"error":{"code":40,"message":"Wrong username or password"}}}"""
        assertEquals(40, StreamReply.subsonicCode(json))
        val jsonMessageFirst = """{"subsonic-response":{"status":"failed","error":{"message":"Song not found","code":70}}}"""
        assertEquals(70, StreamReply.subsonicCode(jsonMessageFirst))
        val xml = """<?xml version="1.0" encoding="UTF-8"?><subsonic-response xmlns="http://subsonic.org/restapi" status="failed" version="1.16.1"><error code="41" message="Token authentication not supported"/></subsonic-response>"""
        assertEquals(41, StreamReply.subsonicCode(xml))
        assertNull(StreamReply.subsonicCode("<html><body>Bad gateway</body></html>"))
        assertNull(StreamReply.subsonicCode(""))
    }

    @Test
    fun errorCodesMapToRecoveryKinds() {
        assertEquals(FailureKind.Auth, StreamReply.kindOf(40))
        assertEquals(FailureKind.Auth, StreamReply.kindOf(41))
        assertEquals(FailureKind.Auth, StreamReply.kindOf(44))
        assertEquals(FailureKind.NotFound, StreamReply.kindOf(70))
        assertEquals(FailureKind.Other, StreamReply.kindOf(0))
        assertEquals(FailureKind.Other, StreamReply.kindOf(50))
        assertEquals(FailureKind.Other, StreamReply.kindOf(null))
    }

    @Test
    fun classifyTakesTheErrorReplyOverEverythingElse() {
        // Before: a JSON error stored as the song failed as a decoding error: Other, never Auth.
        assertEquals(FailureKind.Other, PlaybackRecovery.classify(null, networkIo = false))
        assertEquals(FailureKind.Auth, PlaybackRecovery.classify(null, networkIo = true, errorReply = true, subsonicCode = 40))
        assertEquals(FailureKind.NotFound, PlaybackRecovery.classify(null, networkIo = true, errorReply = true, subsonicCode = 70))
        assertEquals(FailureKind.Other, PlaybackRecovery.classify(null, networkIo = true, errorReply = true))
        // Unchanged without one.
        assertEquals(FailureKind.Auth, PlaybackRecovery.classify(401, networkIo = false))
        assertEquals(FailureKind.Retryable, PlaybackRecovery.classify(null, networkIo = true))
    }

    @Test
    fun aStoredErrorAnswerIsRecognised() {
        // Navidrome's JSON error is about 150 bytes.
        assertTrue(StreamReply.looksLikeErrorReply(150, null))
        assertTrue(StreamReply.looksLikeErrorReply(150, 'x'.code))
        assertTrue(StreamReply.looksLikeErrorReply(5_000, '{'.code))
        assertTrue(StreamReply.looksLikeErrorReply(5_000, '<'.code))
        // Audio: fLaC, OggS, ID3, an MP3 frame.
        assertFalse(StreamReply.looksLikeErrorReply(5_000, 'f'.code))
        assertFalse(StreamReply.looksLikeErrorReply(5_000, 'O'.code))
        assertFalse(StreamReply.looksLikeErrorReply(30_000_000, 'I'.code))
        assertFalse(StreamReply.looksLikeErrorReply(30_000_000, 0xFF))
        assertFalse(StreamReply.looksLikeErrorReply(30_000_000, null))
        assertTrue(StreamReply.needsFirstByte(5_000))
        assertFalse(StreamReply.needsFirstByte(100))
        assertFalse(StreamReply.needsFirstByte(30_000_000))
    }
}
