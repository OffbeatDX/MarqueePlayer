package com.offbeat.marquee.player.net

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import com.offbeat.marquee.player.diag.AppLog
import androidx.compose.runtime.Immutable
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The default network as the app sees it. [online]: it has internet, isn't suspended or a captive
 * portal, and has not lost the system's validation (a network that was validated and then failed
 * re-validation is a dead link: a tunnel, a dead zone). A network that hasn't validated yet counts as
 * online: a slow link can take a long time to pass the probe and still carry music.
 */
@Immutable
data class NetState(val online: Boolean, val unmetered: Boolean)

/**
 * Whether there is a usable network, from ConnectivityManager's default-network callbacks (no
 * polling). [cameBack] fires once on each offline-to-online change (screens refresh then, the
 * outbox flushes, failed prefetches and playback retry); [switched] fires when the default network
 * becomes a different one (connections opened on the old one are dead: the player drops them).
 */
object NetworkMonitor {
    private const val TAG = "MarqueeNet"
    private val _state = MutableStateFlow(NetState(online = true, unmetered = true))
    val state: StateFlow<NetState> = _state.asStateFlow()
    private val _cameBack = MutableSharedFlow<Unit>(extraBufferCapacity = 1)
    val cameBack: SharedFlow<Unit> = _cameBack.asSharedFlow()
    private val _switched = MutableSharedFlow<Unit>(extraBufferCapacity = 1)
    val switched: SharedFlow<Unit> = _switched.asSharedFlow()

    val online: Boolean get() = _state.value.online
    val unmetered: Boolean get() = _state.value.online && _state.value.unmetered

    private var started = false
    private var current: Network? = null

    @Synchronized
    fun init(context: Context) {
        if (started) return
        started = true
        val cm = context.applicationContext.getSystemService(ConnectivityManager::class.java)
        val net = cm.activeNetwork
        current = net
        _state.value = stateOf(net, net?.let(cm::getNetworkCapabilities))
        AppLog.i(TAG, "start: ${describe(_state.value)}")
        cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) = update(network, caps)
            override fun onLost(network: Network) {
                if (network == current) update(null, null)
            }
        })
    }

    @Synchronized
    private fun update(network: Network?, caps: NetworkCapabilities?) {
        val next = stateOf(network, caps)
        val before = _state.value
        val other = network != null && current != null && network != current
        if (network != null || caps == null) current = network
        if (next == before && !other) return
        _state.value = next
        AppLog.i(TAG, "${describe(next)}${if (other) " (another network)" else ""}")
        if (other || (!before.online && next.online)) _switched.tryEmit(Unit)
        if (!before.online && next.online) _cameBack.tryEmit(Unit)
    }

    /** Networks that passed validation at some point (a later loss of it means a dead link). */
    private val validatedOnce = HashSet<Network>()

    private fun stateOf(network: Network?, caps: NetworkCapabilities?): NetState {
        if (caps == null || network == null) return NetState(online = false, unmetered = false)
        val validated = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
        if (validated) validatedOnce += network
        val online = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
            caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_SUSPENDED) &&
            !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_CAPTIVE_PORTAL) &&
            (validated || network !in validatedOnce)
        val unmetered = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) ||
            caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_TEMPORARILY_NOT_METERED)
        return NetState(online, unmetered)
    }

    private fun describe(s: NetState) = if (!s.online) "offline" else if (s.unmetered) "online, unmetered" else "online, metered"
}
