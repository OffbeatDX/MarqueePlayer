package com.offbeat.marquee.player

import android.Manifest
import android.app.Application
import androidx.lifecycle.AndroidViewModel
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.offbeat.marquee.core.settings.AccentChoice
import com.offbeat.marquee.player.art.animatedAccent
import com.offbeat.marquee.player.playback.PlayerConnection
import com.offbeat.marquee.player.session.SessionState
import com.offbeat.marquee.player.session.SessionViewModel
import com.offbeat.marquee.player.settings.UiSettings
import com.offbeat.marquee.player.ui.LocalPlayer
import com.offbeat.marquee.player.ui.SignInScreen
import com.offbeat.marquee.player.ui.SignedInApp
import com.offbeat.marquee.player.ui.theme.MarqueeTheme
import com.offbeat.marquee.player.ui.theme.Palette

/**
 * The UI's connection to the playback service, kept across rotations (and other configuration
 * changes): the new activity has the queue and the playing track from its first frame, so the mini
 * player and Now Playing don't vanish and come back, and nothing below them jumps.
 */
class PlayerHolder(app: Application) : AndroidViewModel(app) {
    val player = PlayerConnection(app)

    override fun onCleared() = player.disconnect()
}

class MainActivity : ComponentActivity() {
    private val session: SessionViewModel by viewModels()
    private val holder: PlayerHolder by viewModels()
    private val player: PlayerConnection get() = holder.player

    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    override fun onCreate(savedInstanceState: Bundle?) {
        // Always dark, so the system bars get light icons whatever the system theme is.
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
        )
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) handleOpen(intent)
        player.onBeforePlay = ::askForNotificationsOnce
        setContent {
            val np by player.state.collectAsStateWithLifecycle()
            val pureBlack by UiSettings.pureBlack.collectAsStateWithLifecycle()
            val accentChoice by UiSettings.accent.collectAsStateWithLifecycle()
            // The accent follows the playing cover (fallback when nothing plays or it has no art), or is fixed.
            val accent = animatedAccent(if (np.hasItem) np.coverArt else null, (accentChoice as? AccentChoice.Fixed)?.rgb)
            MarqueeTheme(accentRgb = accent, pureBlack = pureBlack) {
                CompositionLocalProvider(LocalPlayer provides player) {
                    Surface(Modifier.fillMaxSize(), color = Palette.current.bg) {
                        val state by session.state.collectAsStateWithLifecycle()
                        when (val s = state) {
                            SessionState.Starting -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                                CircularProgressIndicator()
                            }
                            SessionState.SignedOut -> {
                                val form by session.form.collectAsStateWithLifecycle()
                                SignInScreen(form, session::onUrl, session::onUser, session::onPassword, session::signIn)
                            }
                            is SessionState.SignedIn -> SignedInApp(s, player, onSignOut = session::signOut)
                        }
                    }
                }
            }
        }
    }

    /** The media notification needs it on Android 13+; asked once, on the first play. */
    private fun askForNotificationsOnce() {
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) return
        val prefs = getSharedPreferences("ui", Context.MODE_PRIVATE)
        if (prefs.getBoolean(KEY_ASKED_NOTIFICATIONS, false)) return
        prefs.edit().putBoolean(KEY_ASKED_NOTIFICATIONS, true).apply()
        notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleOpen(intent)
    }

    /** The widget and the notification open Now Playing. */
    private fun handleOpen(intent: Intent?) {
        if (intent?.getStringExtra(EXTRA_OPEN) == OPEN_NOW_PLAYING) {
            UiRequests.openNowPlaying.value = true
            intent.removeExtra(EXTRA_OPEN)
        }
    }

    override fun onStart() {
        super.onStart()
        player.connect()
    }

    override fun onStop() {
        // A rotation keeps the connection (the holder outlives this activity); leaving the app drops it.
        if (!isChangingConfigurations) player.disconnect()
        super.onStop()
    }

    override fun onDestroy() {
        // The holder outlives a rotation: it must not keep this activity through the callback.
        player.onBeforePlay = null
        super.onDestroy()
    }

    companion object {
        private const val KEY_ASKED_NOTIFICATIONS = "asked_notifications"
        private const val EXTRA_OPEN = "marquee.open"
        private const val OPEN_NOW_PLAYING = "now_playing"

        /** Opens the app on Now Playing (the widget, the notification). */
        fun nowPlayingIntent(context: Context): Intent =
            Intent(context, MainActivity::class.java)
                .setFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                .putExtra(EXTRA_OPEN, OPEN_NOW_PLAYING)
    }
}

/** Requests from outside the activity's UI, consumed by the screens. */
object UiRequests {
    /** Set when the widget or notification asked for Now Playing; the scaffold opens it and clears this. */
    val openNowPlaying = kotlinx.coroutines.flow.MutableStateFlow(false)
}
