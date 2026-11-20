package com.offbeat.marquee.core.eq

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

@Serializable
enum class EqMode {
    @SerialName("graphic") Graphic,
    @SerialName("profile") Profile,
}

/** The one imported AutoEq / Equalizer APO profile. [preampDb] null: automatic. */
@Serializable
data class EqProfile(
    val name: String,
    val bands: List<Band>,
    val preampDb: Double? = null,
    val notes: List<String> = emptyList(),
)

/**
 * The equalizer setting as it is stored: off by default, the ten sliders, and at most one imported
 * profile (importing another replaces it). [mode] says which of the two plays.
 */
@Serializable
data class EqSetting(
    val enabled: Boolean = false,
    val mode: EqMode = EqMode.Graphic,
    val gains: List<Double> = Graphic.FLAT,
    val profile: EqProfile? = null,
) {
    /** What plays: the profile in profile mode (when there is one), the sliders otherwise. */
    val playsProfile: Boolean get() = mode == EqMode.Profile && profile != null

    val bands: List<Band> get() = if (playsProfile) profile!!.bands else Graphic.bands(gains)

    val preamp: Preamp get() = profile?.preampDb?.takeIf { playsProfile }?.let { Preamp.Manual(it) } ?: Preamp.Auto

    fun params(): EqParams = EqParams(enabled, bands, preamp)

    /** "Bass boost", "Custom", or the profile's name. */
    val label: String get() = if (playsProfile) profile!!.name else Graphic.presetOf(gains)?.name ?: "Custom"

    fun encode(): String = JSON.encodeToString(serializer(), this)

    companion object {
        private val JSON = Json { ignoreUnknownKeys = true; encodeDefaults = true }

        /** A stored setting, or the default (off, flat) when there is none or it can't be read. */
        fun decode(text: String?): EqSetting {
            if (text.isNullOrBlank()) return EqSetting()
            return runCatching { JSON.decodeFromString(serializer(), text) }.getOrNull()?.sanitized() ?: EqSetting()
        }
    }

    private fun sanitized(): EqSetting = copy(
        gains = List(Graphic.COUNT) { i -> gains.getOrElse(i) { 0.0 }.let { if (it.isFinite()) it.coerceIn(-Graphic.MAX_DB, Graphic.MAX_DB) else 0.0 } },
        profile = profile?.takeIf { it.bands.isNotEmpty() }?.let { p ->
            p.copy(
                bands = p.bands.take(MAX_BANDS).map { it.clamped() },
                preampDb = p.preampDb?.let { if (it.isFinite()) it.coerceIn(PREAMP_MIN, PREAMP_MAX) else 0.0 },
            )
        },
    )
}
