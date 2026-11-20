package com.offbeat.marquee.player.widget

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.os.Handler
import android.os.Looper
import com.offbeat.marquee.player.diag.AppLog
import android.content.BroadcastReceiver
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import kotlinx.coroutines.flow.MutableStateFlow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.glance.ColorFilter
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.Image
import androidx.glance.ImageProvider
import androidx.glance.LocalContext
import androidx.glance.LocalSize
import androidx.glance.action.Action
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.action.actionSendBroadcast
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.background
import androidx.glance.layout.Alignment
import androidx.glance.layout.Box
import androidx.glance.layout.Column
import androidx.glance.layout.ContentScale
import androidx.glance.layout.Row
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxSize
import androidx.glance.layout.fillMaxWidth
import androidx.glance.layout.height
import androidx.glance.layout.padding
import androidx.glance.layout.size
import androidx.glance.layout.width
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import androidx.glance.unit.ColorProvider
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import com.offbeat.marquee.core.art.Accent
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.glance.appwidget.updateAll
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.launch
import com.offbeat.marquee.player.MainActivity
import com.offbeat.marquee.player.R
import com.offbeat.marquee.player.art.Art
import com.offbeat.marquee.player.playback.PlaybackService
import com.offbeat.marquee.player.session.AppSession
import com.offbeat.marquee.player.settings.UiSettings
import com.offbeat.marquee.player.ui.theme.Tokens
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The home-screen widget, in the app's dark look with the playing cover's accent. Compact (4x1):
 * cover, title and artist, play/pause, next. Larger (4x2 and up): a bigger cover with previous,
 * play/pause and next. Tapping it opens Now Playing; the buttons go to the playback service through
 * [WidgetButtonReceiver]. Drawn only when [WidgetPusher] says something changed: no timer, no
 * progress bar.
 */
class PlayerWidget : GlanceAppWidget() {
    // Exact: the larger layout sizes its cover to the widget (composed again only when it is resized).
    override val sizeMode = SizeMode.Exact

    /**
     * Glance keeps a session (and its composition) alive for a while after drawing, and an update
     * during that time recomposes it without calling this again: so the content reads [WidgetLive],
     * which the pusher fills before it asks for an update. A new session (the process restarted, the
     * widget just placed) starts from the saved state.
     */
    override suspend fun provideGlance(context: Context, id: GlanceId) {
        if (WidgetLive.data.value == null) WidgetLive.data.value = WidgetLive.load(context, WidgetStore.load(context))
        provideContent {
            val d by WidgetLive.data.collectAsState()
            d?.let {
                SideEffect { AppLog.i(TAG, "widget drawn: ${it.describe()}") }
                Content(it.state, it.cover, it.accent, it.pureBlack)
            }
        }
    }

    companion object {
        private const val TAG = "MarqueeWidget"
        /** From this height on, the larger layout (4x2 and up). */
        val LARGE_FROM = 100.dp
    }
}

/** What the widget draws: the state with its cover bitmap, the accent and pure black, ready. */
class WidgetData(val state: WidgetState, val cover: Bitmap?, val accent: Int, val pureBlack: Boolean) {
    fun describe() = "${if (!state.hasItem) "empty" else if (state.playing) "playing" else "paused"}, cover ${cover?.let { "${it.width}px" } ?: "none"}, " +
        "accent ${Accent.format(accent)}${if (pureBlack) ", pure black" else ""}"
}

object WidgetLive {
    val data = MutableStateFlow<WidgetData?>(null)

    /** The cover (300 bucket) and the accent for [state], from the caches (fetched once when missing). */
    suspend fun load(context: Context, state: WidgetState): WidgetData = withContext(Dispatchers.IO) {
        AppSession.ensureLoaded(context)
        val bmp = runCatching { WidgetStore.cover(context, state.coverId) }.getOrNull()
        val fromCover = if (state.hasItem) runCatching { Art.accent(state.coverId) }.getOrDefault(Accent.FALLBACK) else Accent.FALLBACK
        WidgetData(state, bmp, UiSettings.accent.value.effective(fromCover), UiSettings.pureBlack.value)
    }
}

/**
 * The widget follows Settings > Appearance: when pure black or the accent choice changes (and only
 * then), the placed widgets are drawn again in the new look. Nothing runs otherwise.
 */
object WidgetLook {
    private const val TAG = "MarqueeWidget"

    fun follow(context: Context) {
        val app = context.applicationContext
        CoroutineScope(SupervisorJob() + Dispatchers.Main).launch {
            combine(UiSettings.pureBlack, UiSettings.accent) { black, accent -> black to accent }
                .drop(1)
                .distinctUntilChanged()
                .collect { (black, accent) ->
                    val placed = runCatching { GlanceAppWidgetManager(app).getGlanceIds(PlayerWidget::class.java) }.getOrDefault(emptyList())
                    if (placed.isEmpty()) return@collect
                    // The accent from the caches (the cover's is on disk once computed); the cover bitmap again.
                    WidgetLive.data.value = WidgetLive.load(app, WidgetLive.data.value?.state ?: WidgetStore.load(app))
                    AppLog.i(TAG, "widget push: look changed (${if (black) "pure black" else "dark"}, accent ${accent.encode()}, ${placed.size} placed)")
                    PlayerWidget().updateAll(app)
                }
        }
    }
}

class PlayerWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget: GlanceAppWidget = PlayerWidget()
}

private fun Int.rgb() = Color(0xFF000000.toInt() or this)

@Composable
private fun Content(s: WidgetState, cover: Bitmap?, accentRgb: Int, pureBlack: Boolean) {
    val context = LocalContext.current
    val size = LocalSize.current
    val large = size.height >= PlayerWidget.LARGE_FROM
    val inner = size.height - 16.dp
    // Beside the titles and buttons, the cover fills the height; when the widget is taller than that
    // leaves room for (4x3), the buttons get a row of their own under the cover and the titles.
    val besideCover = minOf(inner, size.width * 0.42f, 180.dp)
    val stacked = large && inner - besideCover > 24.dp
    // Two rows of the launcher's grid are often only ~100 dp: the titles and the buttons share them.
    val tight = inner < 112.dp
    val bg = if (pureBlack) Tokens.BlackSurface else Tokens.Surface
    val accent = accentRgb.rgb()
    val onAccent = Accent.onAccent(accentRgb).rgb()
    val open = actionStartActivity(MainActivity.nowPlayingIntent(context))
    val title = if (s.hasItem) s.title else "Nothing playing"
    val subtitle = if (s.hasItem) s.artist else "Tap play to resume"
    val playLabel = if (s.playing) "Pause" else "Play"
    val playIcon = if (s.playing) R.drawable.widget_pause else R.drawable.widget_play
    val play = WidgetButtonReceiver.action(context, WidgetButtonReceiver.ACTION_PLAY_PAUSE)
    val previous = WidgetButtonReceiver.action(context, WidgetButtonReceiver.ACTION_PREVIOUS)
    val next = WidgetButtonReceiver.action(context, WidgetButtonReceiver.ACTION_NEXT)

    Box(GlanceModifier.fillMaxSize().cornerRadius(16.dp).background(bg).clickable(open).padding(8.dp)) {
        when {
            !large -> Row(GlanceModifier.fillMaxSize(), verticalAlignment = Alignment.CenterVertically) {
                Cover(cover, 48.dp)
                Spacer(GlanceModifier.width(12.dp))
                Titles(title, subtitle, GlanceModifier.defaultWeight(), maxTitleLines = 1)
                Button(playIcon, playLabel, play, 44.dp, accent, onAccent)
                Spacer(GlanceModifier.width(4.dp))
                Button(R.drawable.widget_next, "Next", next, 44.dp, null, Tokens.TextDim)
            }
            !stacked -> Row(GlanceModifier.fillMaxSize(), verticalAlignment = Alignment.CenterVertically) {
                Cover(cover, besideCover)
                Spacer(GlanceModifier.width(14.dp))
                Column(GlanceModifier.defaultWeight(), verticalAlignment = Alignment.CenterVertically) {
                    Titles(title, subtitle, GlanceModifier, maxTitleLines = if (inner >= 120.dp) 2 else 1)
                    Spacer(GlanceModifier.height(if (tight) 2.dp else 8.dp))
                    Row(verticalAlignment = Alignment.CenterVertically) { Controls(previous, play, next, playIcon, playLabel, accent, onAccent, tight) }
                }
            }
            else -> {
                val buttons = 56.dp
                val top = inner - buttons - 8.dp
                val coverSize = minOf(top, size.width * 0.45f, 220.dp)
                Column(GlanceModifier.fillMaxSize()) {
                    Row(GlanceModifier.fillMaxWidth().height(top), verticalAlignment = Alignment.CenterVertically) {
                        Cover(cover, coverSize)
                        Spacer(GlanceModifier.width(14.dp))
                        Titles(title, subtitle, GlanceModifier.defaultWeight(), maxTitleLines = 3)
                    }
                    Spacer(GlanceModifier.height(8.dp))
                    Row(
                        GlanceModifier.fillMaxWidth().height(buttons),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) { Controls(previous, play, next, playIcon, playLabel, accent, onAccent, tight = false) }
                }
            }
        }
    }
}

@Composable
private fun Controls(previous: Action, play: Action, next: Action, playIcon: Int, playLabel: String, accent: Color, onAccent: Color, tight: Boolean) {
    val side = if (tight) 44.dp else 48.dp
    Button(R.drawable.widget_previous, "Previous", previous, side, null, Tokens.TextDim)
    Spacer(GlanceModifier.width(8.dp))
    Button(playIcon, playLabel, play, if (tight) 44.dp else 52.dp, accent, onAccent)
    Spacer(GlanceModifier.width(8.dp))
    Button(R.drawable.widget_next, "Next", next, side, null, Tokens.TextDim)
}

@Composable
private fun Cover(cover: Bitmap?, size: Dp) {
    if (cover != null) {
        Image(
            provider = ImageProvider(cover),
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = GlanceModifier.size(size).cornerRadius(8.dp),
        )
    } else {
        Box(GlanceModifier.size(size).cornerRadius(8.dp).background(Tokens.SurfaceHi), contentAlignment = Alignment.Center) {
            Image(
                provider = ImageProvider(R.drawable.widget_album),
                contentDescription = null,
                colorFilter = ColorFilter.tint(ColorProvider(Tokens.TextFaint)),
                modifier = GlanceModifier.size(size / 2),
            )
        }
    }
}

@Composable
private fun Titles(title: String, subtitle: String, modifier: GlanceModifier, maxTitleLines: Int) {
    Column(modifier) {
        Text(title, maxLines = maxTitleLines, style = TextStyle(color = ColorProvider(Tokens.Text), fontSize = 15.sp, fontWeight = FontWeight.Bold))
        if (subtitle.isNotEmpty()) {
            Text(subtitle, maxLines = 1, style = TextStyle(color = ColorProvider(Tokens.TextDim), fontSize = 13.sp))
        }
    }
}

@Composable
private fun Button(icon: Int, label: String, action: Action, size: Dp, fill: Color?, tint: Color) {
    var m = GlanceModifier.size(size)
    if (fill != null) m = m.cornerRadius(size / 2).background(fill)
    Box(m.clickable(action), contentAlignment = Alignment.Center) {
        Image(
            provider = ImageProvider(icon),
            contentDescription = label,
            colorFilter = ColorFilter.tint(ColorProvider(tint)),
            modifier = GlanceModifier.size(24.dp),
        )
    }
}

/**
 * The widget's buttons. Media3's MediaButtonReceiver takes only "play" on Android 8+, so this one
 * connects a MediaController to the session for each press: play/pause (with an empty queue, play
 * resumes the saved one through `onPlaybackResumption`), next, previous. The press comes from the
 * widget, which lets the service go to the foreground when playback starts.
 */
class WidgetButtonReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action ?: return
        if (action !in ACTIONS) return
        val app = context.applicationContext
        val pending = goAsync()
        val main = Handler(Looper.getMainLooper())
        val token = SessionToken(app, ComponentName(app, PlaybackService::class.java))
        val future = MediaController.Builder(app, token).buildAsync()
        var done = false
        val finish = {
            if (!done) {
                done = true
                MediaController.releaseFuture(future)
                pending.finish()
            }
        }
        main.postDelayed(finish, TIMEOUT_MS)
        future.addListener({
            val c = runCatching { future.get() }.getOrNull()
            if (c == null) {
                AppLog.w(TAG, "widget button: no session")
                finish()
                return@addListener
            }
            val before = if (c.playWhenReady) "playing" else "paused"
            when (action) {
                ACTION_PLAY_PAUSE -> if (c.playWhenReady && c.playbackState != Player.STATE_ENDED) c.pause() else c.play()
                ACTION_NEXT -> c.seekToNext()
                ACTION_PREVIOUS -> c.seekToPrevious()
            }
            AppLog.i(TAG, "widget button ${action.substringAfterLast('.')} (was $before, ${c.mediaItemCount} queued)")
            // Let the commands reach the session before the controller goes.
            main.postDelayed(finish, 500)
        }, ContextCompat.getMainExecutor(app))
    }

    companion object {
        private const val TAG = "MarqueeWidget"
        private const val TIMEOUT_MS = 8_000L
        const val ACTION_PLAY_PAUSE = "com.offbeat.marquee.widget.PLAY_PAUSE"
        const val ACTION_NEXT = "com.offbeat.marquee.widget.NEXT"
        const val ACTION_PREVIOUS = "com.offbeat.marquee.widget.PREVIOUS"
        private val ACTIONS = setOf(ACTION_PLAY_PAUSE, ACTION_NEXT, ACTION_PREVIOUS)

        fun action(context: Context, what: String): Action =
            actionSendBroadcast(Intent(what).setComponent(ComponentName(context, WidgetButtonReceiver::class.java)))
    }
}
