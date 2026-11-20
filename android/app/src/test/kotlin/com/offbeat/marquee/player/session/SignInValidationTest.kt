package com.offbeat.marquee.player.session

import com.offbeat.marquee.core.subsonic.SubsonicError
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SignInValidationTest {
    @Test
    fun addressWithCredentialsGetsItsOwnErrorMessage() {
        val credsMsg = errorMessage(SubsonicError.InvalidUrl(hasCredentials = true))
        val genericMsg = errorMessage(SubsonicError.InvalidUrl())
        assertNotEquals(genericMsg, credsMsg)
        assertTrue(credsMsg.contains("username", ignoreCase = true))
        assertTrue(credsMsg.contains("password", ignoreCase = true))

        // Rejected whether or not the separate username/password fields are filled.
        val (url1, err1) = validateSignIn(SignInForm(url = "https://me:secret@music.example.com", user = "me", password = "pw"))
        assertNull(url1)
        assertEquals(credsMsg, err1)

        val (url2, err2) = validateSignIn(SignInForm(url = "me:secret@music.example.com"))
        assertNull(url2)
        assertEquals(credsMsg, err2)

        val (url3, err3) = validateSignIn(SignInForm(url = "https://me@music.example.com", user = "me", password = "pw"))
        assertNull(url3)
        assertEquals(credsMsg, err3)
    }

    @Test
    fun malformedAndValidAddressesStillBehaveAsBefore() {
        val (badUrl, badErr) = validateSignIn(SignInForm(url = "ftp://music.example.com", user = "me", password = "pw"))
        assertNull(badUrl)
        assertEquals(errorMessage(SubsonicError.InvalidUrl()), badErr)

        val (goodUrl, goodErr) = validateSignIn(SignInForm(url = "music.example.com/nd/rest/", user = "me", password = "pw"))
        assertNull(goodErr)
        assertNotNull(goodUrl)
        assertEquals("https://music.example.com/nd", goodUrl.toString())
    }
}
