//! macOS: an FSEvents stream on the folder, per-file events (`kFSEventStreamCreateFlagFileEvents`),
//! delivered on a serial dispatch queue of its own (the stream's thread is the system's: nothing
//! here waits or polls). FSEvents always watches the whole tree: a flat watch drops what is
//! deeper than the folder's own entries. The stream reports real paths (`/private/var/...` for a
//! folder under `/var`); they are given back under the path the watch was asked for.

use std::ffi::{CStr, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2_core_foundation::{CFArray, CFString};
use objc2_core_services::{
    ConstFSEventStreamRef, FSEventStreamContext, FSEventStreamCreate, FSEventStreamEventFlags, FSEventStreamEventId,
    FSEventStreamInvalidate, FSEventStreamRef, FSEventStreamRelease, FSEventStreamSetDispatchQueue,
    FSEventStreamStart, FSEventStreamStop, kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagNoDefer,
    kFSEventStreamCreateFlagWatchRoot, kFSEventStreamEventFlagKernelDropped, kFSEventStreamEventFlagMustScanSubDirs,
    kFSEventStreamEventFlagRootChanged, kFSEventStreamEventFlagUnmount, kFSEventStreamEventFlagUserDropped,
    kFSEventStreamEventIdSinceNow,
};

use super::Seen;

/// How long FSEvents gathers events before handing them over, in seconds. With `NoDefer`,
/// the first of a burst comes at once and the rest after this.
const LATENCY: f64 = 0.05;

/// What the callback needs: the queue is serial, so it is only ever used by one callback at
/// a time, and freed only once the queue has run dry (`Inner::stop`).
struct State {
    /// The folder as the caller named it, and as FSEvents names it (its real path).
    root: PathBuf,
    real: PathBuf,
    recursive: bool,
    on: Mutex<Box<dyn FnMut(Seen) + Send>>,
    ended: AtomicBool,
}

impl State {
    /// An event's path, under the folder as the caller named it.
    fn path(&self, real: &Path) -> PathBuf {
        match real.strip_prefix(&self.real) {
            Ok(rest) if rest.as_os_str().is_empty() => self.root.clone(),
            Ok(rest) => self.root.join(rest),
            Err(_) => real.to_path_buf(),
        }
    }

    fn send(&self, seen: Seen) {
        if self.ended.load(Ordering::Acquire) {
            return;
        }
        if seen == Seen::Ended {
            self.ended.store(true, Ordering::Release);
        }
        let mut on = self.on.lock().unwrap_or_else(|e| e.into_inner());
        // A panic in the caller's handler must not unwind into the system's queue.
        if catch_unwind(AssertUnwindSafe(|| on(seen))).is_err() {
            self.ended.store(true, Ordering::Release);
        }
    }

    fn event(&self, real: &Path, flags: FSEventStreamEventFlags) {
        if flags & (kFSEventStreamEventFlagRootChanged | kFSEventStreamEventFlagUnmount) != 0 {
            // The folder (or a folder above it) was moved, deleted or unmounted.
            if self.root.is_dir() {
                self.send(Seen::Lost);
            } else {
                self.send(Seen::Ended);
            }
            return;
        }
        if flags & (kFSEventStreamEventFlagMustScanSubDirs | kFSEventStreamEventFlagUserDropped | kFSEventStreamEventFlagKernelDropped) != 0 {
            self.send(Seen::Lost);
            return;
        }
        let path = self.path(real);
        if path == self.root {
            // About the folder itself: gone, or (still there) its own creation or metadata,
            // which FSEvents also tells of when it happened just before the stream started.
            // What changes inside it comes with its own path.
            if !self.root.is_dir() {
                self.send(Seen::Ended);
            }
            return;
        }
        if !self.recursive && path.parent() != Some(self.root.as_path()) {
            return;
        }
        self.send(Seen::Path(path));
    }
}

unsafe extern "C-unwind" fn callback(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: NonNull<c_void>,
    flags: NonNull<FSEventStreamEventFlags>,
    _ids: NonNull<FSEventStreamEventId>,
) {
    // SAFETY: `info` is the `State` given at creation, alive until the queue has run dry.
    let Some(state) = (unsafe { info.cast::<State>().as_ref() }) else { return };
    // SAFETY: without `UseCFTypes`, `paths` is `count` C strings, `flags` `count` flags.
    let (paths, flags) = unsafe {
        (std::slice::from_raw_parts(paths.as_ptr().cast::<*const c_char>(), count), std::slice::from_raw_parts(flags.as_ptr(), count))
    };
    for (&p, &f) in paths.iter().zip(flags) {
        if p.is_null() {
            continue;
        }
        // SAFETY: a NUL-terminated path from FSEvents.
        let bytes = unsafe { CStr::from_ptr(p) }.to_bytes();
        state.event(Path::new(std::ffi::OsStr::from_bytes(bytes)), f);
    }
}

pub struct Inner {
    stream: FSEventStreamRef,
    queue: DispatchRetained<DispatchQueue>,
    state: *mut State,
}

// The stream and its state are only touched by the queue's callbacks and by `stop`, which
// stops the stream and waits for the queue before freeing anything.
unsafe impl Send for Inner {}

pub fn start(path: &Path, recursive: bool, on: Box<dyn FnMut(Seen) + Send>) -> std::io::Result<Inner> {
    if !std::fs::metadata(path)?.is_dir() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotADirectory, "not a folder"));
    }
    let real = std::fs::canonicalize(path)?;
    let name = real.to_str().ok_or_else(|| std::io::Error::other("the folder's path is not UTF-8"))?;
    let paths = CFArray::from_retained_objects(&[CFString::from_str(name)]);
    let state = Box::into_raw(Box::new(State { root: path.to_path_buf(), real, recursive, on: Mutex::new(on), ended: AtomicBool::new(false) }));
    let mut context = FSEventStreamContext { version: 0, info: state.cast(), retain: None, release: None, copyDescription: None };
    let flags = kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer | kFSEventStreamCreateFlagWatchRoot;
    // SAFETY: a valid callback, context and array of CFStrings; the context is copied.
    let stream = unsafe {
        FSEventStreamCreate(None, Some(callback), &mut context, paths.as_opaque(), kFSEventStreamEventIdSinceNow, LATENCY, flags)
    };
    if stream.is_null() {
        // SAFETY: from `Box::into_raw` above, never handed to a stream.
        drop(unsafe { Box::from_raw(state) });
        return Err(std::io::Error::other("FSEventStreamCreate failed"));
    }
    let queue = DispatchQueue::new("marquee.dirwatch", DispatchQueueAttr::SERIAL);
    // SAFETY: a stream just created, scheduled once, on a queue that outlives it.
    unsafe { FSEventStreamSetDispatchQueue(stream, Some(&queue)) };
    let inner = Inner { stream, queue, state };
    // SAFETY: a scheduled stream.
    if !unsafe { FSEventStreamStart(stream) } {
        inner.stop();
        return Err(std::io::Error::other("FSEventStreamStart failed"));
    }
    Ok(inner)
}

impl Inner {
    pub fn stop(self) {
        // SAFETY: the stream from `start`, stopped, invalidated and released once.
        unsafe {
            FSEventStreamStop(self.stream);
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
        }
        // A callback already queued runs before this returns; none comes after it.
        self.queue.exec_sync(|| {});
        // SAFETY: from `Box::into_raw` in `start`; the queue no longer uses it.
        drop(unsafe { Box::from_raw(self.state) });
    }
}
