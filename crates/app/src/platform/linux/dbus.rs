//! The session bus, shared: one connection for the app's own calls (the file manager, the
//! tray host, the desktop's settings portal). zbus (pure Rust, the same the toolkit's
//! accessibility and the tray already use); its connection sleeps on the socket between
//! messages. `None` when there is no session bus (a bare X11 or headless session).
//!
//! The questions below (`has_owner`, `property`, `portal_setting`) are asked on the UI thread,
//! so they go on a second connection whose calls give up after `QUICK`: a hung portal or tray
//! host costs the window a moment once a minute at most, not its life. `session` has no such limit, for calls made
//! on a thread of their own that may wait for a program to start (the file manager).

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use zbus::blocking::Connection;

/// How long a question asked on the UI thread waits for its answer.
const QUICK: Duration = Duration::from_millis(1000);

pub fn session() -> Option<&'static Connection> {
    static BUS: OnceLock<Option<Connection>> = OnceLock::new();
    BUS.get_or_init(|| match Connection::session() {
        Ok(c) => Some(c),
        Err(e) => {
            log::info!("no D-Bus session bus ({e})");
            None
        }
    })
    .as_ref()
}

/// The connection for the UI thread's questions: each call waits at most `QUICK`.
fn quick() -> Option<&'static Connection> {
    static BUS: OnceLock<Option<Connection>> = OnceLock::new();
    BUS.get_or_init(|| match zbus::blocking::connection::Builder::session().map(|b| b.method_timeout(QUICK)).and_then(|b| b.build()) {
        Ok(c) => Some(c),
        Err(e) => {
            log::info!("no D-Bus session bus ({e})");
            None
        }
    })
    .as_ref()
}

/// A question on the quick connection. A program that did not answer in time is not asked again
/// for a minute (the tray glyph asks every few seconds while music plays).
fn ask<B>(dest: &str, path: &str, iface: &str, method: &str, body: &B) -> Option<zbus::message::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    static STALLED: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
    let bus = quick()?;
    {
        let mut stalled = STALLED.lock().unwrap();
        stalled.retain(|(_, at)| at.elapsed() < Duration::from_secs(60));
        if stalled.iter().any(|(d, _)| d == dest) {
            return None;
        }
    }
    let t = Instant::now();
    let r = bus.call_method(Some(dest), path, Some(iface), method, body);
    if r.is_err() && t.elapsed() >= QUICK {
        log::info!("D-Bus: {dest} did not answer {iface}.{method} in {} ms; not asked again for a minute", QUICK.as_millis());
        STALLED.lock().unwrap().push((dest.to_owned(), Instant::now()));
    }
    r.ok()
}

/// Whether a bus name has an owner now.
pub fn has_owner(name: &str) -> bool {
    ask("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", &(name,)).and_then(|m| m.body().deserialize::<bool>().ok()).unwrap_or(false)
}

/// A property of an object, read with `org.freedesktop.DBus.Properties.Get`.
pub fn property<T>(name: &str, path: &str, iface: &str, prop: &str) -> Option<T>
where
    T: TryFrom<zbus::zvariant::OwnedValue>,
{
    let m = ask(name, path, "org.freedesktop.DBus.Properties", "Get", &(iface, prop))?;
    let v: zbus::zvariant::OwnedValue = m.body().deserialize().ok()?;
    T::try_from(v).ok()
}

/// A value of the desktop's settings portal (`org.freedesktop.portal.Settings.ReadOne`),
/// for the colour scheme and reduced motion. `None` without the portal.
pub fn portal_setting<T>(namespace: &str, key: &str) -> Option<T>
where
    T: TryFrom<zbus::zvariant::OwnedValue>,
{
    if !has_owner("org.freedesktop.portal.Desktop") {
        return None;
    }
    let m = ask("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop", "org.freedesktop.portal.Settings", "ReadOne", &(namespace, key))?;
    let v: zbus::zvariant::OwnedValue = m.body().deserialize().ok()?;
    T::try_from(v).ok()
}
