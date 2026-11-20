//! Windows: WinHTTP, Windows' own HTTP stack and TLS (the system's proxy settings and
//! certificate store), so the exe carries no HTTP or TLS library. Synchronous calls on the
//! caller's thread; redirects followed (WinHTTP's default policy: never from https to http)
//! unless the caller follows them itself;
//! no decompression asked for, so a body comes as the server sent it.
//!
//! One WinHTTP session per process (per user agent, which a session carries: the few callers
//! each have their own), opened at the first request and kept: its proxy settings found once,
//! its connections kept alive between requests. Each request sets its own timeouts.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::ptr::{null, null_mut};
use std::sync::Mutex;

use windows_sys::Win32::Networking::WinHttp::*;

use super::{CHUNK, Head, HttpError, Request, Sink, parse_retry_after, rate_limit, split_url};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The error of the last WinHTTP call, in words: WinHTTP's codes (12001-12185) are not in
/// the system's message table, so the common ones are named here.
fn last(what: &str) -> HttpError {
    let e = std::io::Error::last_os_error();
    let words = match e.raw_os_error().unwrap_or(0) as u32 {
        ERROR_WINHTTP_TIMEOUT => "timed out".to_owned(),
        ERROR_WINHTTP_NAME_NOT_RESOLVED => "the server's name could not be resolved".to_owned(),
        ERROR_WINHTTP_CANNOT_CONNECT => "could not connect".to_owned(),
        ERROR_WINHTTP_CONNECTION_ERROR => "the connection was reset or closed".to_owned(),
        ERROR_WINHTTP_INVALID_SERVER_RESPONSE => "the server's response could not be read".to_owned(),
        ERROR_WINHTTP_SECURE_FAILURE => "the secure connection failed (certificate or TLS)".to_owned(),
        ERROR_WINHTTP_REDIRECT_FAILED => "a redirect failed".to_owned(),
        ERROR_WINHTTP_OPERATION_CANCELLED => "cancelled".to_owned(),
        ERROR_WINHTTP_INVALID_URL | ERROR_WINHTTP_UNRECOGNIZED_SCHEME => "not a valid URL".to_owned(),
        code @ 12000..=12999 => format!("WinHTTP error {code}"),
        _ => e.to_string(),
    };
    HttpError::Network(format!("{what}: {words}"))
}

struct H(*mut core::ffi::c_void);

impl Drop for H {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a handle WinHTTP gave us, closed once. Closing a request with body left
            // unread drops its connection at once.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// The process's session for this user agent, opened the first time (a handle, kept open for
/// the process's life: WinHTTP's session handles may be used from any thread at once).
fn session(agent: &str) -> Result<*mut core::ffi::c_void, HttpError> {
    static SESSIONS: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);
    let mut g = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(&h) = map.get(agent) {
        return Ok(h as *mut core::ffi::c_void);
    }
    let agent_w = wide(agent);
    // SAFETY: a NUL-terminated UTF-16 string that outlives the call; the handle is never closed.
    let h = unsafe { WinHttpOpen(agent_w.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0) };
    if h.is_null() {
        return Err(last("WinHttpOpen"));
    }
    map.insert(agent.to_owned(), h as usize);
    Ok(h)
}

/// One response header as text, None when the response has none.
///
/// SAFETY: `request` is an open request handle whose response has been received.
unsafe fn header(request: &H, info: u32) -> Option<String> {
    // SAFETY: as this function's.
    unsafe { query(request, info, null()) }
}

/// A header by name (`WINHTTP_QUERY_CUSTOM`), None when the response has none.
///
/// SAFETY: as `header`'s.
unsafe fn custom_header(request: &H, name: &str) -> Option<String> {
    let name = wide(name);
    // SAFETY: `name` is NUL-terminated and outlives the call.
    unsafe { query(request, WINHTTP_QUERY_CUSTOM, name.as_ptr()) }
}

/// SAFETY: `request` as `header`'s; `name` null or a NUL-terminated UTF-16 string.
unsafe fn query(request: &H, info: u32, name: *const u16) -> Option<String> {
    let mut buf = vec![0u16; 128];
    loop {
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: the buffer is `size` bytes long.
        let ok = unsafe { WinHttpQueryHeaders(request.0, info, name, buf.as_mut_ptr() as _, &mut size, null_mut()) };
        if ok != 0 {
            let n = (size as usize / 2).min(buf.len());
            return Some(String::from_utf16_lossy(&buf[..n]).trim().to_owned());
        }
        // Too small: `size` now says how many bytes it needs. Anything else: no such header.
        if std::io::Error::last_os_error().raw_os_error() != Some(windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER as i32) || buf.len() >= 1 << 16 {
            return None;
        }
        buf.resize(size as usize / 2 + 1, 0);
    }
}

/// Send `req` and give the body to `sink` as it comes; (the head, whether the sink stopped).
/// `follow`: WinHTTP follows redirects; otherwise a redirect comes back as it is, with its `Location`.
pub(super) fn download(req: &Request, sink: &mut dyn Sink, follow: bool) -> Result<(Head, bool), HttpError> {
    let (secure, host, port, path) = split_url(req.url)?;
    // WinHttpConnect takes an IPv6 address without its brackets.
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let session = session(req.user_agent)?;
    // SAFETY: plain WinHTTP calls with NUL-terminated UTF-16 strings and buffers that
    // outlive them; every handle but the session's is closed by `H`.
    unsafe {
        let host_w = wide(host);
        let conn = H(WinHttpConnect(session, host_w.as_ptr(), port, 0));
        if conn.0.is_null() {
            return Err(last("connect"));
        }
        let verb = wide(req.method);
        let path_w = wide(&path);
        let request = H(WinHttpOpenRequest(conn.0, verb.as_ptr(), path_w.as_ptr(), null(), null(), null(), if secure { WINHTTP_FLAG_SECURE } else { 0 }));
        if request.0.is_null() {
            return Err(last("open request"));
        }
        // This request's own timeouts (the session is shared). The receive timeout bounds each
        // wait for the response and each read of the body, not the whole download.
        let t = req.timeout_ms as i32;
        WinHttpSetTimeouts(request.0, t, t, t, t);
        if !follow {
            let never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
            if WinHttpSetOption(request.0, WINHTTP_OPTION_REDIRECT_POLICY, &never as *const u32 as _, 4) == 0 {
                return Err(last("redirect policy"));
            }
        }
        let headers: String = req.headers.iter().map(|h| format!("{h}\r\n")).collect();
        let headers_w = wide(&headers);
        let (hp, hl) = if headers.is_empty() { (null(), 0) } else { (headers_w.as_ptr(), u32::MAX) };
        let body = req.body;
        let bp = if body.is_empty() { null() } else { body.as_ptr() as _ };
        if WinHttpSendRequest(request.0, hp, hl, bp, body.len() as u32, body.len() as u32, 0) == 0 {
            return Err(last("send"));
        }
        if WinHttpReceiveResponse(request.0, null_mut()) == 0 {
            return Err(last("receive"));
        }
        let mut status: u32 = 0;
        let mut size = 4u32;
        WinHttpQueryHeaders(request.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, null(), &mut status as *mut u32 as _, &mut size, null_mut());
        let head = Head {
            status: status as u16,
            // As text: the number form is 32 bits, and a file can be larger.
            content_length: header(&request, WINHTTP_QUERY_CONTENT_LENGTH).and_then(|s| s.parse().ok()),
            content_range: header(&request, WINHTTP_QUERY_CONTENT_RANGE),
            content_type: header(&request, WINHTTP_QUERY_CONTENT_TYPE),
            retry_after: header(&request, WINHTTP_QUERY_RETRY_AFTER).and_then(|s| parse_retry_after(&s)),
            rate_limit: rate_limit(|name| custom_header(&request, name)),
            location: if follow { None } else { header(&request, WINHTTP_QUERY_LOCATION) },
        };
        if sink.head(&head).is_break() {
            return Ok((head, true));
        }
        let mut buf = vec![0u8; CHUNK];
        loop {
            // Waits (up to the receive timeout) until some of the body is in, or its end.
            let mut avail = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut avail) == 0 {
                return Err(last("read"));
            }
            if avail == 0 {
                break;
            }
            let want = (avail as usize).min(buf.len()) as u32;
            let mut read = 0u32;
            if WinHttpReadData(request.0, buf.as_mut_ptr() as _, want, &mut read) == 0 {
                return Err(last("read"));
            }
            if read == 0 {
                break;
            }
            if let ControlFlow::Break(()) = sink.chunk(&buf[..read as usize]) {
                return Ok((head, true));
            }
        }
        Ok((head, false))
    }
}
