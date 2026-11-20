//! Linux (and any system without WinHTTP): ureq, a small blocking client, with rustls for
//! TLS (the `ring` provider) and certificates checked against the system's root store
//! (`rustls-platform-verifier`: on Linux the distribution's CA bundle, as every other
//! program there uses). Proxies come from `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY` and
//! `NO_PROXY`, as is the convention there. Statuses are answers, not errors: a 404 or a
//! 429 comes back as a `Head`, as WinHTTP gives it. When `follow` is true, ureq follows up
//! to 10 redirects to any address (including https to http); signed-in requests (carrying an
//! `Authorization` header, or made via the `*_same_server` calls such as Subsonic's) pass
//! `follow = false` and are followed by `follow_same_server` in `lib.rs` instead, which
//! refuses scheme changes and other hosts or ports. Without ureq's `gzip` feature nothing is
//! decompressed: a body comes as sent.

use std::io::Read;
use std::ops::ControlFlow;
use std::sync::OnceLock;
use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig};

use super::{CHUNK, Head, HttpError, Request, Sink, parse_retry_after, rate_limit, split_url};

/// One agent per timeout: it keeps connections for reuse, as WinHTTP's session does not,
/// but each caller (the scrobbler, the lyrics lookup, the Subsonic client) passes the same
/// few timeouts.
fn agent(timeout_ms: u32) -> ureq::Agent {
    static AGENTS: OnceLock<std::sync::Mutex<Vec<(u32, ureq::Agent)>>> = OnceLock::new();
    let agents = AGENTS.get_or_init(Default::default);
    let mut list = agents.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, a)) = list.iter().find(|(t, _)| *t == timeout_ms) {
        return a.clone();
    }
    let t = Duration::from_millis(u64::from(timeout_ms));
    // The body is timed per read, as WinHTTP times it, not as a whole: a song of several
    // hundred MB may take minutes, a stalled one fails after `t`. (The buffered calls are
    // bounded by `MAX_BODY` and `MAX_TIME` instead.)
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_resolve(Some(t))
        .timeout_connect(Some(t))
        .timeout_send_request(Some(t))
        .timeout_recv_response(Some(t))
        .timeout_per_read(Some(t))
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build();
    let a: ureq::Agent = config.into();
    list.push((timeout_ms, a.clone()));
    a
}

/// A ureq error in words, without the URL a few of them repeat (it may carry a token).
fn net(e: ureq::Error) -> HttpError {
    HttpError::Network(match e {
        ureq::Error::BadUri(_) => "not a valid URL".to_owned(),
        ureq::Error::RequireHttpsOnly(_) => "https is required".to_owned(),
        ureq::Error::ConnectProxyFailed(_) => "the proxy refused the connection".to_owned(),
        ureq::Error::Http(_) => "the request could not be built".to_owned(),
        e => e.to_string(),
    })
}

/// Send `req` and give the body to `sink` as it comes; (the head, whether the sink stopped).
/// `follow`: ureq follows redirects; otherwise a redirect comes back as it is, with its `Location`.
pub(super) fn download(req: &Request, sink: &mut dyn Sink, follow: bool) -> Result<(Head, bool), HttpError> {
    let redirects = if follow { 10 } else { 0 };
    // The same URLs as WinHTTP takes, and the same message for one it does not.
    split_url(req.url)?;
    let agent = agent(req.timeout_ms);
    let mut headers: Vec<(&str, &str)> = vec![("User-Agent", req.user_agent)];
    for h in req.headers {
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim(), v.trim()));
        }
    }
    let response = match req.method {
        "GET" | "HEAD" | "DELETE" => {
            let mut r = match req.method {
                "GET" => agent.get(req.url),
                "HEAD" => agent.head(req.url),
                _ => agent.delete(req.url),
            };
            for (k, v) in &headers {
                r = r.header(*k, *v);
            }
            r.config().max_redirects(redirects).build().call().map_err(net)?
        }
        method => {
            let mut r = match method {
                "POST" => agent.post(req.url),
                "PUT" => agent.put(req.url),
                "PATCH" => agent.patch(req.url),
                m => return Err(HttpError::Network(format!("method {m} is not supported"))),
            };
            for (k, v) in &headers {
                r = r.header(*k, *v);
            }
            r.config().max_redirects(redirects).build().send(req.body).map_err(net)?
        }
    };
    let text = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_owned());
    let head = Head {
        status: response.status().as_u16(),
        content_length: response.body().content_length(),
        content_range: text("content-range"),
        content_type: text("content-type"),
        retry_after: text("retry-after").and_then(|s| parse_retry_after(&s)),
        rate_limit: rate_limit(|name| text(&name.to_ascii_lowercase())),
        location: if follow { None } else { text("location") },
    };
    if sink.head(&head).is_break() {
        return Ok((head, true));
    }
    // Unlimited: the caller decides how much it keeps (the buffered calls stop at MAX_BODY).
    // Dropping the reader early drops the connection, without reading the rest.
    let mut reader = response.into_body().into_reader();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok((head, false)),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(HttpError::Network(format!("read: {e}"))),
        };
        if let ControlFlow::Break(()) = sink.chunk(&buf[..n]) {
            return Ok((head, true));
        }
    }
}
