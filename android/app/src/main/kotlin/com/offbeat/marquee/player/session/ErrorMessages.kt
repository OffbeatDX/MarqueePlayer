package com.offbeat.marquee.player.session

import com.offbeat.marquee.core.subsonic.SubsonicError
import java.net.UnknownServiceException

/** What the user reads for each client error. Never includes the address or credentials. */
fun errorMessage(e: SubsonicError): String = when (e) {
    is SubsonicError.InvalidUrl -> if (e.hasCredentials) {
        "Don't put a username or password in the server address. Enter them below instead."
    } else {
        "That doesn't look like a server address."
    }
    is SubsonicError.Network -> when {
        !com.offbeat.marquee.player.net.NetworkMonitor.online -> "You're offline, and this isn't saved on the phone yet."
        e.cause is UnknownServiceException -> "Plain http isn't allowed. Use an https:// address."
        e.kind == SubsonicError.Network.Kind.UnknownHost -> "Can't find that server. Check the address and your connection."
        e.kind == SubsonicError.Network.Kind.Timeout -> "The server didn't answer in time."
        e.kind == SubsonicError.Network.Kind.Tls -> "Couldn't make a secure connection to the server."
        else -> "Can't reach the server. Check the address and your connection."
    }
    is SubsonicError.AuthFailed -> when (e.code) {
        40 -> "Wrong username or password."
        41 -> "This server doesn't accept token sign-in."
        else -> "The server refused the sign-in (error ${e.code})."
    }
    is SubsonicError.NotSubsonic -> "That address answered, but it isn't a Subsonic or Navidrome server."
    is SubsonicError.Server -> when {
        e.code != null -> "The server reported an error (${e.code})."
        e.httpStatus != null -> "The server reported an error (HTTP ${e.httpStatus})."
        else -> "The server reported an error."
    }
    is SubsonicError.RefusedReadOnly -> "Blocked: this build is read-only (${e.endpoint})."
}
