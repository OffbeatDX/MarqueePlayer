package com.offbeat.marquee.core.auto

import com.offbeat.marquee.core.auto.ControllerTrust.Grant
import org.junit.Assert.assertEquals
import org.junit.Test

class ControllerTrustTest {
    private fun grant(
        pkg: String,
        sameApp: Boolean = false,
        trusted: Boolean = false,
        notification: Boolean = false,
        auto: Boolean = false,
    ) = ControllerTrust.grantFor(sameApp, trusted, notification, auto, pkg)

    @Test
    fun anyOtherAppOnlyPlaysAndPauses() {
        // Before: every package got the heart (a server write), the queue commands, the covers and the browse tree.
        assertEquals(Grant.PlayerOnly, grant("com.example.someapp"))
        assertEquals(Grant.PlayerOnly, grant("android.media.session.MediaController"))
    }

    @Test
    fun theAppTheSystemAndTheCarAreTrusted() {
        assertEquals(Grant.Full, grant("com.offbeat.marquee.player", sameApp = true))
        assertEquals(Grant.Full, grant("com.android.bluetooth", trusted = true))
        assertEquals(Grant.Full, grant("com.example.launcher", notification = true))
        assertEquals(Grant.Full, grant("com.google.android.projection.gearhead", auto = true))
        assertEquals(Grant.Full, grant("com.android.systemui"))
        assertEquals(Grant.Full, grant("com.google.android.googlequicksearchbox"))
        assertEquals(Grant.Full, grant("com.google.android.wearable.app"))
    }
}
