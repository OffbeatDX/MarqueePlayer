//! What can go wrong, sorted into what the UI says and does about it. No message here ever
//! holds a request URL, the token, the salt or the password: URLs are never put in one,
//! and text that comes from elsewhere (the HTTP layer, the server's own error message) has
//! the token and salt masked by the client before it lands here.

/// A failed call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// What was typed cannot be a server address ([`crate::normalize_url`]).
    BadUrl,
    /// No answer: offline, DNS, refused, TLS, a timeout, a connection cut mid-answer. Worth
    /// trying again later.
    Network(String),
    /// Something answered, but not a Subsonic server: a web page, a 404, JSON of another
    /// kind, garbage. Usually the wrong address (or a path such as "/navidrome" left out).
    NotSubsonic(String),
    /// Subsonic codes 40-44: 40 wrong user or password; 41 token authentication not
    /// supported for this user (an LDAP account on some servers); 42-44 OpenSubsonic's
    /// other authentication mechanisms. The user must sign in again.
    WrongCredentials { code: i64, message: String },
    /// Code 50: the user is not allowed this (a write without the role, a playlist of
    /// someone else's).
    NotPermitted(String),
    /// Code 70: no such song, album, artist or playlist (removed on the server since).
    NotFound(String),
    /// Any other Subsonic error code, or an HTTP error status around a non-Subsonic answer
    /// (a 5xx from the server or a proxy), with the server's message when it gave one.
    Server { code: Option<i64>, http_status: Option<u16>, message: String },
    /// A Subsonic answer whose content could not be read: a field missing that the call
    /// needs, an item of a type no tolerance covers.
    BadResponse(String),
    /// This call writes (the endpoint) and the client may not send that kind of write (it is
    /// read-only, or that kind is off: `Writes`): refused before any
    /// request was made.
    ReadOnly(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BadUrl => f.write_str("not a valid server address"),
            Error::Network(m) => write!(f, "no connection to the server ({m})"),
            Error::NotSubsonic(m) => write!(f, "not a Subsonic server ({m})"),
            Error::WrongCredentials { code, message } => write!(f, "wrong user name or password (Subsonic error {code}: {message})"),
            Error::NotPermitted(m) => write!(f, "not permitted ({m})"),
            Error::NotFound(m) => write!(f, "not found on the server ({m})"),
            Error::Server { code, http_status, message } => {
                f.write_str("server error")?;
                match (code, http_status) {
                    (Some(c), _) => write!(f, " (Subsonic error {c}")?,
                    (None, Some(s)) => write!(f, " (HTTP {s}")?,
                    (None, None) => f.write_str(" (")?,
                }
                if message.is_empty() { f.write_str(")") } else { write!(f, ": {message})") }
            }
            Error::BadResponse(m) => write!(f, "the server's answer could not be read ({m})"),
            Error::ReadOnly(endpoint) => write!(f, "{endpoint} refused: the connection does not send this kind of write"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// The class for a Subsonic error code.
    pub fn from_code(code: i64, message: String, http_status: Option<u16>) -> Self {
        match code {
            40..=44 => Error::WrongCredentials { code, message },
            50 => Error::NotPermitted(message),
            70 => Error::NotFound(message),
            _ => Error::Server { code: Some(code), http_status, message },
        }
    }

    /// Worth trying again later, unchanged: no connection, or the server (or a proxy before
    /// it) failing with a 5xx.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Network(_) => true,
            Error::Server { http_status: Some(s), .. } => (500..600).contains(s),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_sort_into_classes() {
        assert!(matches!(Error::from_code(40, "x".into(), None), Error::WrongCredentials { code: 40, .. }));
        assert!(matches!(Error::from_code(41, "x".into(), None), Error::WrongCredentials { code: 41, .. }));
        assert_eq!(Error::from_code(50, "x".into(), None), Error::NotPermitted("x".into()));
        assert_eq!(Error::from_code(70, "x".into(), None), Error::NotFound("x".into()));
        assert_eq!(Error::from_code(10, "x".into(), None), Error::Server { code: Some(10), http_status: None, message: "x".into() });
        assert!(Error::Network("x".into()).is_retryable());
        assert!(Error::Server { code: None, http_status: Some(503), message: String::new() }.is_retryable());
        assert!(!Error::Server { code: Some(0), http_status: None, message: String::new() }.is_retryable());
        assert!(!Error::NotFound("x".into()).is_retryable());
        assert_eq!(Error::Server { code: None, http_status: Some(502), message: String::new() }.to_string(), "server error (HTTP 502)");
    }
}
