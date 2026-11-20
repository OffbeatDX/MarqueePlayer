package com.offbeat.marquee.core.subsonic

import java.security.MessageDigest
import java.security.SecureRandom

/**
 * What the app keeps to talk to the server: the base URL, the user, and a salt with its token
 * (md5(password + salt)). The password itself is never stored.
 *
 * [toString] hides the salt and token, so a credentials object can't leak through a log line.
 */
data class SubsonicCredentials(
    val url: String,
    val user: String,
    val salt: String,
    val token: String,
) {
    override fun toString(): String = "SubsonicCredentials(user=$user, url=<hidden>, salt=<hidden>, token=<hidden>)"

    companion object {
        /** Makes a fresh salt and its token from the [password] the user typed at sign-in. */
        fun fromPassword(url: String, user: String, password: String, random: SecureRandom = SecureRandom()): SubsonicCredentials {
            val salt = SubsonicAuth.newSalt(random)
            return SubsonicCredentials(url.trim(), user.trim(), salt, SubsonicAuth.token(password, salt))
        }
    }
}

object SubsonicAuth {
    /** 16 random bytes as 32 hex characters. */
    fun newSalt(random: SecureRandom = SecureRandom()): String {
        val bytes = ByteArray(16)
        random.nextBytes(bytes)
        return bytes.toHex()
    }

    /** The Subsonic token: lowercase hex md5 of password followed by salt. */
    fun token(password: String, salt: String): String =
        MessageDigest.getInstance("MD5").digest((password + salt).toByteArray(Charsets.UTF_8)).toHex()

    private fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }
}
