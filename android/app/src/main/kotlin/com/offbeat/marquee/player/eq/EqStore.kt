package com.offbeat.marquee.player.eq

import android.content.Context
import android.content.SharedPreferences
import com.offbeat.marquee.core.eq.EqSetting
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The equalizer setting (off by default), shared by the screen and [com.offbeat.marquee.player.playback.PlaybackService]
 * in this one process, stored as JSON in SharedPreferences. While a slider is dragged the setting changes
 * live (the audio thread takes it up at its next buffer) and is written once the drag ends.
 */
object EqStore {
    private lateinit var prefs: SharedPreferences
    private val _setting = MutableStateFlow(EqSetting())
    val setting: StateFlow<EqSetting> = _setting.asStateFlow()

    fun init(context: Context) {
        prefs = context.applicationContext.getSharedPreferences("eq", Context.MODE_PRIVATE)
        _setting.value = EqSetting.decode(prefs.getString(KEY, null))
    }

    /** Change the setting; [persist] false while a slider is still moving. */
    fun update(persist: Boolean = true, change: (EqSetting) -> EqSetting) {
        val next = change(_setting.value)
        _setting.value = next
        if (persist) save()
    }

    fun save() {
        prefs.edit().putString(KEY, _setting.value.encode()).apply()
    }

    private const val KEY = "setting"
}
