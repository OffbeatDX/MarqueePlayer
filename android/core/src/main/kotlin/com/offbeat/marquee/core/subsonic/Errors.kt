package com.offbeat.marquee.core.subsonic

/**
 * Everything the client throws. Messages never contain the server URL or any auth parameter.
 */
sealed class SubsonicError(message: String, cause: Throwable? = null) : Exception(message, cause) {
    /** The server could not be reached: no network, DNS, TLS, timeout. */
    class Network(val kind: Kind, cause: Throwable?) : SubsonicError("Network error ($kind)", cause) {
        enum class Kind { UnknownHost, Timeout, Tls, Other }
    }

    /** The server answered, but rejected the user or token (Subsonic codes 40-44). */
    class AuthFailed(val code: Int, serverMessage: String?) : SubsonicError("Authentication failed ($code): ${serverMessage.orEmpty()}")

    /** The server answered with a Subsonic error, or an HTTP error status. */
    class Server(val code: Int?, val httpStatus: Int?, serverMessage: String?) :
        SubsonicError("Server error (code=$code, http=$httpStatus): ${serverMessage.orEmpty()}")

    /** What was typed cannot be a server address (or embeds a username/password). */
    class InvalidUrl(val hasCredentials: Boolean = false) : SubsonicError(
        if (hasCredentials) "Server address cannot contain a username or password"
        else "Not a valid server address",
    )

    /** The address answered, but not with a Subsonic response. */
    class NotSubsonic(detail: String) : SubsonicError("Not a Subsonic server: $detail")

    /** Read-only mode refused the call locally; nothing was sent. */
    class RefusedReadOnly(val endpoint: String) : SubsonicError("Refused by read-only mode: $endpoint")
}
