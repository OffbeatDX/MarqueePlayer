package com.offbeat.marquee.core.auto

/**
 * What a controller connecting to the media session may do. Any app on the phone can connect; only
 * those the system vouches for, or that are the app itself, get more than playing and pausing:
 *
 * - [Grant.Full]: the app's own controllers (same uid: the screens, the widget), controllers the
 *   system trusts (Media3's `isTrusted`: holding MEDIA_CONTENT_CONTROL or an enabled notification
 *   listener, as the system UI and Bluetooth do), the media notification's, Android Auto and
 *   Automotive, and a short list of known system packages (the system UI, Assistant, Wear's
 *   companion). They get the browse tree, the custom commands (the heart, which writes to the server,
 *   the queue and sleep-timer commands) and read access to the covers.
 * - [Grant.PlayerOnly]: anyone else: the default player commands, no browse tree, no custom
 *   commands, no cover grant.
 */
object ControllerTrust {
    enum class Grant { Full, PlayerOnly }

    val KNOWN_PACKAGES: Set<String> = setOf(
        "com.android.systemui",
        "com.google.android.projection.gearhead", // Android Auto
        "com.google.android.carassistant", // Assistant on Automotive
        "com.google.android.googlequicksearchbox", // Assistant
        "com.google.android.wearable.app", // Wear OS companion
        "com.google.android.apps.wear.companion", // Wear OS companion (Pixel Watch)
    )

    fun grantFor(
        sameApp: Boolean,
        trustedBySystem: Boolean,
        mediaNotification: Boolean,
        autoOrAutomotive: Boolean,
        packageName: String,
    ): Grant = when {
        sameApp || trustedBySystem || mediaNotification || autoOrAutomotive -> Grant.Full
        packageName in KNOWN_PACKAGES -> Grant.Full
        else -> Grant.PlayerOnly
    }
}
