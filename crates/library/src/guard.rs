//! Panics this crate catches and recovers from itself (one bad file must not take a scan or
//! the player down). The app's panic hook asks `active()` so that a panic caught here is
//! logged with its backtrace but is not the fatal error an uncaught one is.

use std::cell::Cell;

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Whether this thread is inside `catching` (a panic now will be caught and recovered from).
pub fn active() -> bool {
    DEPTH.with(Cell::get) > 0
}

/// Run `f`, turning a panic in it into `Err` with the panic's message.
pub fn catching<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    DEPTH.with(|d| d.set(d.get() + 1));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    DEPTH.with(|d| d.set(d.get() - 1));
    r.map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned())
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_panic_becomes_its_message_and_the_depth_unwinds() {
        assert!(!super::active());
        assert_eq!(super::catching(|| 7), Ok(7));
        assert_eq!(super::catching(|| -> i32 { panic!("boom {}", 3) }), Err("boom 3".to_owned()));
        assert!(super::catching(super::active).unwrap());
        assert!(!super::active());
    }
}
