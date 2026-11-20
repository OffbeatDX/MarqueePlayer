//! How often the engine's threads wake, for the app's once-a-minute diagnostics line
//! (`MP_LOG=debug`, the app's `wakes.rs`). Relaxed counters, one add each: the device callback
//! counts too (an atomic add does not block or allocate).

use std::sync::atomic::{AtomicU64, Ordering};

/// Turns of the output thread's loop (each wait it returns from, each chunk it takes).
pub static OUTPUT_LOOP: AtomicU64 = AtomicU64::new(0);
/// Callbacks of a pull device (Core Audio, PipeWire): each one wakes the output thread.
pub static CALLBACKS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn note(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

/// A counter's value now.
pub fn get(c: &AtomicU64) -> u64 {
    c.load(Ordering::Relaxed)
}
