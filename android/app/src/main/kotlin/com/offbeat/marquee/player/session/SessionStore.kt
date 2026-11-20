package com.offbeat.marquee.player.session

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Keeps the signed-in server (url, user, salt, token; never the password), encrypted with an
 * AES-GCM key that lives in the Android Keystore and never leaves it.
 */
class SessionStore(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences("session", Context.MODE_PRIVATE)

    fun load(): SubsonicCredentials? {
        val blob = prefs.getString(KEY_BLOB, null) ?: return null
        return try {
            val (ivB64, ctB64) = blob.split(':', limit = 2)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, Base64.decode(ivB64, Base64.NO_WRAP)))
            val plain = cipher.doFinal(Base64.decode(ctB64, Base64.NO_WRAP))
            json.decodeFromString(Stored.serializer(), plain.toString(Charsets.UTF_8)).toCredentials()
        } catch (e: Exception) {
            // The key is gone (app data restored elsewhere, Keystore reset) or the blob is damaged:
            // the only way forward is signing in again.
            clear()
            null
        }
    }

    fun save(credentials: SubsonicCredentials) {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val plain = json.encodeToString(Stored.serializer(), Stored.of(credentials)).toByteArray(Charsets.UTF_8)
        val ct = cipher.doFinal(plain)
        val blob = Base64.encodeToString(cipher.iv, Base64.NO_WRAP) + ":" + Base64.encodeToString(ct, Base64.NO_WRAP)
        prefs.edit().putString(KEY_BLOB, blob).apply()
    }

    fun clear() {
        prefs.edit().remove(KEY_BLOB).apply()
    }

    private fun key(): SecretKey {
        val ks = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (ks.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        gen.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return gen.generateKey()
    }

    @Serializable
    private data class Stored(val url: String, val user: String, val salt: String, val token: String) {
        fun toCredentials() = SubsonicCredentials(url, user, salt, token)

        override fun toString() = "Stored(<hidden>)"

        companion object {
            fun of(c: SubsonicCredentials) = Stored(c.url, c.user, c.salt, c.token)
        }
    }

    private companion object {
        const val KEYSTORE = "AndroidKeyStore"
        const val ALIAS = "marquee_session"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val TAG_BITS = 128
        const val KEY_BLOB = "credentials"
        val json = Json { ignoreUnknownKeys = true }
    }
}
