package com.offbeat.marquee.player.harness

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Debug only: crashes the app on purpose, to check that the crash reaches the on-phone log and
 * `last-crash.txt` ("Share diagnostics"):
 * `adb -s emulator-5580 shell am broadcast -n com.offbeat.marquee.player.debug/com.offbeat.marquee.player.harness.CrashTestReceiver`
 */
class CrashTestReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        throw IllegalStateException("Test crash (debug build, CrashTestReceiver)")
    }
}
