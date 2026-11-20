package com.offbeat.marquee.player.session

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import coil3.SingletonImageLoader
import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.BuildConfig
import com.offbeat.marquee.player.art.Art
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.diag.AppLog
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.sync.Sync
import com.offbeat.marquee.player.data.LyricsRepo
import com.offbeat.marquee.player.settings.LibraryPrefs
import com.offbeat.marquee.player.settings.UiSettings
import com.offbeat.marquee.player.playback.MediaStores
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.HttpUrl

data class SignInForm(
    val url: String = "",
    val user: String = "",
    val password: String = "",
    val busy: Boolean = false,
    val error: String? = null,
)

sealed interface SessionState {
    data object Starting : SessionState
    data object SignedOut : SessionState
    data class SignedIn(val readOnly: Boolean, val testServer: Boolean) : SessionState
}

/** Sign-in and the stored session. Survives rotation. */
class SessionViewModel(app: Application) : AndroidViewModel(app) {
    private val store = SessionStore(app)

    private val _state = MutableStateFlow<SessionState>(SessionState.Starting)
    val state: StateFlow<SessionState> = _state.asStateFlow()

    private val _form = MutableStateFlow(SignInForm())
    val form: StateFlow<SignInForm> = _form.asStateFlow()

    init {
        viewModelScope.launch {
            val (creds, testServer) = withContext(Dispatchers.IO) {
                TestServerConfig.load(app)?.let { it to true } ?: (store.load() to false)
            }
            if (creds == null) {
                _state.value = SessionState.SignedOut
            } else {
                // A pushed test server is always read-only; so is every debug build.
                startSession(creds, readOnly = testServer || BuildConfig.READ_ONLY, testServer = testServer)
            }
        }
    }

    fun onUrl(v: String) = _form.update { it.copy(url = v, error = null) }
    fun onUser(v: String) = _form.update { it.copy(user = v, error = null) }
    fun onPassword(v: String) = _form.update { it.copy(password = v, error = null) }

    fun signIn() {
        val f = _form.value
        if (f.busy) return
        val (normalized, validationError) = validateSignIn(f)
        if (validationError != null || normalized == null) {
            _form.update { it.copy(error = validationError) }
            return
        }
        // The address and user are hidden from the log from now on, before the server answers.
        AppLog.hide(normalized.toString(), f.user)
        _form.update { it.copy(busy = true, error = null) }
        viewModelScope.launch {
            val creds = SubsonicCredentials.fromPassword(normalized.toString().trimEnd('/'), f.user, f.password)
            try {
                val c = SubsonicClient(creds, readOnly = BuildConfig.READ_ONLY)
                val info = c.ping()
                if (info.status != "ok") throw SubsonicError.NotSubsonic("status ${info.status}")
                try {
                    withContext(Dispatchers.IO) { store.save(creds) }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    // The Keystore can refuse (some devices, after a Keystore reset): say so, don't crash.
                    AppLog.w("Session", "sign-in: the credentials could not be saved", e)
                    _form.update { it.copy(busy = false, password = "", error = "Couldn't save the sign-in on this phone.") }
                    return@launch
                }
                _form.value = SignInForm(url = f.url, user = f.user) // the password is dropped here
                startSession(creds, readOnly = BuildConfig.READ_ONLY, testServer = false)
            } catch (e: SubsonicError) {
                _form.update { it.copy(busy = false, error = errorMessage(e)) }
            }
        }
    }

    fun signOut() {
        AppSession.clear()
        val app = getApplication<Application>()
        viewModelScope.launch(Dispatchers.IO) {
            store.clear()
            TestServerConfig.clear(app)
            // What was kept from this server goes with it.
            Library.clear()
            Downloads.clear()
            Sync.clear()
            LyricsRepo.clear()
            LibraryPrefs.clearRecent()
            UiSettings.setServer(null)
            // The on-phone log and the last crash go too.
            AppLog.clear()
            // The cache of recent and upcoming tracks too (the confirmation says so).
            runCatching { MediaStores.clearCache(app) }
            Art.clear()
            SingletonImageLoader.get(app).let { it.diskCache?.clear(); it.memoryCache?.clear() }
        }
        _form.value = SignInForm()
        _state.value = SessionState.SignedOut
    }

    private fun startSession(creds: SubsonicCredentials, readOnly: Boolean, testServer: Boolean) {
        val c = try {
            SubsonicClient(creds, readOnly = readOnly)
        } catch (e: SubsonicError) {
            _state.value = SessionState.SignedOut
            _form.update { it.copy(error = errorMessage(e)) }
            return
        }
        AppSession.set(c)
        _state.value = SessionState.SignedIn(readOnly = readOnly, testServer = testServer)
    }
}

/** Validates the sign-in form locally; returns the normalised server URL or an error for the screen. */
internal fun validateSignIn(f: SignInForm): Pair<HttpUrl?, String?> {
    if (f.url.isBlank()) return null to "Enter the server address."
    if (SubsonicClient.hasUrlCredentials(f.url)) return null to errorMessage(SubsonicError.InvalidUrl(hasCredentials = true))
    if (f.user.isBlank()) return null to "Enter your username."
    if (f.password.isEmpty()) return null to "Enter your password."
    val normalized = SubsonicClient.normalizeUrl(f.url) ?: return null to errorMessage(SubsonicError.InvalidUrl())
    return normalized to null
}
