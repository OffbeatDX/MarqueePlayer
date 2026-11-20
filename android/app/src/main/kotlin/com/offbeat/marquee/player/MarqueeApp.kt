package com.offbeat.marquee.player

import android.app.Application
import coil3.ImageLoader
import coil3.PlatformContext
import coil3.SingletonImageLoader
import com.offbeat.marquee.player.art.Art
import com.offbeat.marquee.player.art.Images
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.data.LyricsRepo
import com.offbeat.marquee.player.diag.AppLog
import com.offbeat.marquee.player.eq.EqStore
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.offline.Availability
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.sync.Sync
import com.offbeat.marquee.player.playback.MediaButtons
import com.offbeat.marquee.player.playback.PlaybackSettings
import com.offbeat.marquee.player.widget.WidgetLook
import com.offbeat.marquee.player.settings.LibraryPrefs
import com.offbeat.marquee.player.settings.UiSettings

class MarqueeApp : Application(), SingletonImageLoader.Factory {
    override fun onCreate() {
        super.onCreate()
        // First: the crash handler and the on-phone log ("Share diagnostics").
        AppLog.init(this)
        UiSettings.init(this)
        PlaybackSettings.init(this)
        MediaButtons.init(this)
        EqStore.init(this)
        LibraryPrefs.init(this)
        NetworkMonitor.init(this)
        Sync.init(this)
        Library.init(this)
        Downloads.init(this)
        Availability.init(this)
        LyricsRepo.init(this)
        Art.init(this)
        // The widget follows pure black and a fixed accent (pushed only when they change).
        WidgetLook.follow(this)
    }

    /** Coil, on the session's OkHttp client (read-only guard included in debug builds). */
    override fun newImageLoader(context: PlatformContext): ImageLoader = Images.newLoader(context)
}
