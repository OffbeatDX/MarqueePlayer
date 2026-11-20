//! Replacing a music file safely.
//!
//! A write never touches the file itself: it is copied to a temporary file beside it (same
//! folder, so the same volume), the copy is changed and checked, flushed to disk, and only
//! then swapped in with `ReplaceFileW`, which keeps the original's creation time,
//! attributes, security and alternate streams. The original is moved to a backup name by
//! the same call and deleted after; if the swap fails half way, the backup is put back.
//! At every moment the file's name holds either the old file or the new one, complete.
//!
//! Before the temporary file is made, a record of its names goes in the data folder
//! (`inflight/`); it is removed when the write is done. On the next start `recover` looks
//! at any record left by a crash or a power cut: a file whose name is empty gets its backup
//! (the old file) back, and the temporary and backup files are deleted. Nothing else in
//! the music folders is ever looked at or removed.
//!
//! The player reads files with `File::open`, which lets others rename and delete them: a
//! file being played is replaced like any other, the engine reading on from the old data
//! (Windows keeps it until the handle closes) and the next play opening the new file.
//! A program holding a file without that sharing makes the swap fail; nothing changes then.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Why one file could not be written. The file is unchanged in every case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Missing,
    ReadOnly,
    /// Changed on disk since the draft (or the save being undone) was made.
    Changed(String),
    InUse,
    DiskFull,
    Denied,
    /// A drive or share that is not there (any more).
    Unavailable,
    /// The written copy did not check out: it was thrown away.
    Check(String),
    /// Something else (the reason).
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(f, "the file is not there"),
            Self::ReadOnly => write!(f, "the file is read-only"),
            Self::Changed(why) => write!(f, "{why}"),
            Self::InUse => write!(f, "another program is using the file"),
            Self::DiskFull => write!(f, "the disk is full"),
            Self::Denied if cfg!(windows) => write!(f, "Windows does not allow changing this file or its folder"),
            Self::Denied => write!(f, "the system does not allow changing this file or its folder (no permission, or a read-only drive)"),
            Self::Unavailable => write!(f, "its drive or network location is not available"),
            Self::Check(why) => write!(f, "the change was not made because the written copy did not check out: {why}"),
            Self::Other(why) => write!(f, "{why}"),
        }
    }
}

/// An I/O error as a person reads it.
pub fn failure(e: &std::io::Error) -> Failure {
    // Unix: errno.
    #[cfg(unix)]
    return match e.raw_os_error() {
        Some(libc::ENOENT | libc::ENOTDIR) => Failure::Missing,
        Some(libc::EBUSY | libc::ETXTBSY) => Failure::InUse,
        Some(libc::ENOSPC | libc::EDQUOT | libc::EFBIG) => Failure::DiskFull,
        Some(libc::EACCES | libc::EPERM | libc::EROFS) => Failure::Denied,
        Some(
            libc::EIO | libc::ENODEV | libc::ENXIO | libc::ESTALE | libc::ENOTCONN | libc::EHOSTDOWN | libc::EHOSTUNREACH | libc::ETIMEDOUT
            | libc::ENETUNREACH,
        ) => Failure::Unavailable,
        _ => Failure::Other(e.to_string()),
    };
    // Windows: the system error codes.
    #[cfg(not(unix))]
    match e.raw_os_error() {
        Some(2 | 3) => Failure::Missing,
        Some(32 | 33 | 1175 | 1176 | 1177) => Failure::InUse,
        Some(39 | 112) => Failure::DiskFull,
        Some(5 | 19) => Failure::Denied,
        Some(21 | 51 | 53 | 55 | 59 | 64 | 67 | 1222 | 1231 | 1167) => Failure::Unavailable,
        _ => Failure::Other(e.to_string()),
    }
}

// ---- The write guard (tests and the bench) -----------------------------------------------

type GuardFn = fn(&Path) -> bool;
static GUARD: RwLock<Option<GuardFn>> = RwLock::new(None);

/// Refuse every write to a path for which `allow` says false. Tests and the bench set it:
/// they may only write to fixtures they made (a folder with their marker file).
pub fn set_guard(allow: GuardFn) {
    if let Ok(mut g) = GUARD.write() {
        *g = Some(allow);
    }
}

fn guarded(path: &Path) -> Result<(), Failure> {
    let g = GUARD.read().map(|g| *g).unwrap_or(None);
    match g {
        Some(allow) if !allow(path) => Err(Failure::Other(format!("refused by the write guard: {}", path.display()))),
        _ => Ok(()),
    }
}

/// Whether the write guard (if one is set) lets anything write `path`: the Lyrics tab's
/// sidecar files (`lyrics::write_sidecar`) answer to it too.
pub fn allowed(path: &Path) -> bool {
    guarded(path).is_ok()
}

/// A guard for tests and benches: only files under a folder holding `marker`, inside the
/// system's temporary folder.
pub fn under_marker(path: &Path, marker: &str) -> bool {
    let temp = std::env::temp_dir();
    let temp = std::fs::canonicalize(&temp).unwrap_or(temp);
    let Ok(p) = std::fs::canonicalize(path.parent().unwrap_or(path)) else { return false };
    p.starts_with(&temp) && p.ancestors().any(|a| a.join(marker).is_file())
}

// ---- Names --------------------------------------------------------------------------------

/// The suffix of the editor's temporary and backup files ("<name>.mpedit-<token>.tmp").
pub const TEMP_MARK: &str = ".mpedit-";

fn token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mut h = crate::hash::Hasher::new();
    h.number(t);
    h.number(u64::from(std::process::id()));
    h.number(N.fetch_add(1, Ordering::Relaxed));
    format!("{:08x}", h.finish() as u32)
}

/// A file being replaced: its temporary copy and the backup name the original takes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Staged {
    pub original: PathBuf,
    pub temp: PathBuf,
    pub backup: PathBuf,
    /// The record of it in the data folder.
    #[serde(skip)]
    pub record: PathBuf,
}

fn sibling(path: &Path, token: &str, ext: &str) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{name}{TEMP_MARK}{token}.{ext}"))
}

/// A path as `ReplaceFileW` takes it, long-path form when it needs one.
#[cfg(windows)]
fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let s = p.as_os_str().to_string_lossy();
    let long = if p.is_absolute() && s.len() >= 248 && !s.starts_with(r"\\?\") {
        match s.strip_prefix(r"\\") {
            Some(unc) => format!(r"\\?\UNC\{unc}"),
            None => format!(r"\\?\{s}"),
        }
    } else {
        s.into_owned()
    };
    std::ffi::OsStr::new(&long).encode_wide().chain([0]).collect()
}

// ---- Checks before writing ----------------------------------------------------------------

/// The file can be replaced: it is there, a file, not read-only.
pub fn check_target(path: &Path) -> Result<std::fs::Metadata, Failure> {
    guarded(path)?;
    #[cfg(unix)]
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Failure::Other("a symbolic link: edit the file it points to".into()));
    }
    let m = std::fs::metadata(path).map_err(|e| failure(&e))?;
    if !m.is_file() {
        return Err(Failure::Other("not a file".into()));
    }
    if m.permissions().readonly() {
        return Err(Failure::ReadOnly);
    }
    Ok(m)
}

/// Free space on the file's volume, in bytes.
#[cfg(windows)]
fn free_space(dir: &Path) -> Option<u64> {
    let mut free = 0u64;
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide(dir).as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

#[cfg(unix)]
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: a NUL-terminated path, and a plain C struct to fill.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

#[cfg(not(any(windows, unix)))]
fn free_space(_dir: &Path) -> Option<u64> {
    None
}

// ---- Staging, committing, recovering -------------------------------------------------------

/// Copy the file to a temporary file beside it, after leaving a record in `inflight`. The
/// copy is a plain new file (no attributes of the original: it gets those when it
/// replaces it). `extra` is room the write may need beyond the file's size.
pub fn stage(original: &Path, inflight: &Path, extra: u64) -> Result<Staged, Failure> {
    guarded(original)?;
    let size = std::fs::metadata(original).map_err(|e| failure(&e))?.len();
    let dir = original.parent().ok_or(Failure::Other("no folder".into()))?;
    if let Some(free) = free_space(dir)
        && free < size.saturating_add(extra).saturating_add(1 << 20)
    {
        return Err(Failure::DiskFull);
    }
    let t = token();
    let s = Staged { original: original.to_path_buf(), temp: sibling(original, &t, "tmp"), backup: sibling(original, &t, "bak"), record: inflight.join(format!("{t}.json")) };
    std::fs::create_dir_all(inflight).map_err(|e| Failure::Other(format!("the data folder cannot be written ({e})")))?;
    let json = serde_json::to_vec(&s).map_err(|e| Failure::Other(e.to_string()))?;
    write_synced(&s.record, &json).map_err(|e| Failure::Other(format!("the data folder cannot be written ({e})")))?;
    let copied = (|| -> std::io::Result<()> {
        let mut src = std::fs::File::open(original)?;
        let mut dst = std::fs::OpenOptions::new().write(true).create_new(true).open(&s.temp)?;
        std::io::copy(&mut src, &mut dst)?;
        dst.sync_all()
    })();
    if let Err(e) = copied {
        abandon(&s);
        return Err(failure(&e));
    }
    Ok(s)
}

fn write_synced(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// Flush the written copy to the disk (before it may take the original's place).
pub fn flush(s: &Staged) -> Result<(), Failure> {
    std::fs::OpenOptions::new().write(true).open(&s.temp).and_then(|f| f.sync_all()).map_err(|e| failure(&e))
}

/// Give up a staged write: the temporary file and the record go; the original was never
/// touched. Unless a swap that failed left the original at its backup name (it could not be
/// moved back): then the record stays, for `recover` to put it back at the next start.
pub fn abandon(s: &Staged) {
    let _ = std::fs::remove_file(&s.temp);
    if !s.original.exists() && s.backup.exists() {
        log::error!("edit: {} is at {}; put back at the next start", s.original.display(), s.backup.display());
        return;
    }
    let _ = std::fs::remove_file(&s.record);
}

/// Stages a test can stop at, to show what a crash there leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crash {
    /// The copy was written and checked; the original was not touched.
    BeforeReplace,
    /// The original was moved to its backup name; the copy not yet in its place.
    AfterBackup,
    /// The copy is in place; the backup and the record not yet removed.
    AfterReplace,
}

/// Put the checked copy in the original's place.
pub fn commit(s: &Staged) -> Result<(), Failure> {
    commit_with(s, None)
}

/// `commit`, stopping at `crash` (tests): what is left there is what a crash leaves.
pub fn commit_with(s: &Staged, crash: Option<Crash>) -> Result<(), Failure> {
    guarded(&s.original)?;
    flush(s)?;
    if crash == Some(Crash::BeforeReplace) {
        return Err(Failure::Other("simulated crash".into()));
    }
    if crash == Some(Crash::AfterBackup) {
        std::fs::rename(&s.original, &s.backup).map_err(|e| failure(&e))?;
        return Err(Failure::Other("simulated crash".into()));
    }
    replace(s)?;
    if crash == Some(Crash::AfterReplace) {
        return Err(Failure::Other("simulated crash".into()));
    }
    // The old file: gone now (or once the player lets go of it).
    let _ = std::fs::remove_file(&s.backup);
    let _ = std::fs::remove_file(&s.record);
    Ok(())
}

#[cfg(windows)]
fn replace(s: &Staged) -> Result<(), Failure> {
    use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_WRITE_THROUGH, ReplaceFileW};
    let (o, t, b) = (wide(&s.original), wide(&s.temp), wide(&s.backup));
    let ok = unsafe { ReplaceFileW(o.as_ptr(), t.as_ptr(), b.as_ptr(), REPLACEFILE_WRITE_THROUGH, std::ptr::null(), std::ptr::null()) };
    if ok != 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    // A volume whose file system or server cannot do what ReplaceFileW asks of it (some network
    // shares and NAS, FAT drivers without the information classes it uses): nothing was moved,
    // so the same swap by two renames, which the record in `inflight` covers just the same.
    const ERROR_INVALID_FUNCTION: i32 = 1;
    const ERROR_NOT_SUPPORTED: i32 = 50;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    if matches!(err.raw_os_error(), Some(ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED | ERROR_INVALID_PARAMETER)) && s.original.exists() && !s.backup.exists() {
        log::info!("edit: ReplaceFileW is not supported here ({err}); swapping by renames");
        return swap_by_renames(s);
    }
    // ERROR_UNABLE_TO_MOVE_REPLACEMENT(_2): the original may be at the backup name now.
    if !s.original.exists()
        && s.backup.exists()
        && let Err(e) = std::fs::rename(&s.backup, &s.original)
    {
        log::error!("edit: {} is at {} and could not be moved back: {e}", s.original.display(), s.backup.display());
        return Err(Failure::Other(format!("the original is safe as {} ({e})", s.backup.display())));
    }
    let _ = std::fs::remove_file(&s.temp);
    let _ = std::fs::remove_file(&s.record);
    Err(failure(&err))
}

/// Unix: `rename(2)` of the checked copy over the original, after giving the copy what the
/// original had that an unprivileged process can give: its permission bits, its owner and
/// group (when allowed: one's own file, a group one is in) and its extended attributes
/// (`user.*`, an ACL; a security label where permitted). Then the folder is flushed, so the
/// new name survives a power cut. At every moment the name holds the old file or the new
/// one, whole (no backup name is needed, unlike `ReplaceFileW`); a program reading the old
/// file (the player) reads on from it, as the kernel keeps it until it is closed. Another
/// hard link to the file keeps the old content. A symbolic link is refused before anything
/// is written (`check_target`): it would become a plain file. On macOS, Finder's tags and
/// comments are extended attributes and are kept; an ACL, file flags (`chflags`) and the
/// creation date are not.
#[cfg(unix)]
fn replace(s: &Staged) -> Result<(), Failure> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::io::AsRawFd;
    let give_up = |e: std::io::Error| {
        let _ = std::fs::remove_file(&s.temp);
        let _ = std::fs::remove_file(&s.record);
        failure(&e)
    };
    if REFUSE_REPLACE.with(std::cell::Cell::get) {
        // A test's stand-in for a replace the system refuses (`edit::hold`).
        return Err(give_up(std::io::Error::from_raw_os_error(libc::EBUSY)));
    }
    let m = std::fs::metadata(&s.original).map_err(give_up)?;
    let copy = std::fs::OpenOptions::new().write(true).open(&s.temp).map_err(give_up)?;
    let _ = copy.set_permissions(std::fs::Permissions::from_mode(m.mode() & 0o7777));
    // SAFETY: a valid descriptor; refused (EPERM) where the owner cannot be given.
    unsafe { libc::fchown(copy.as_raw_fd(), m.uid(), m.gid()) };
    copy_xattrs(&s.original, &s.temp);
    copy.sync_all().map_err(give_up)?;
    drop(copy);
    std::fs::rename(&s.temp, &s.original).map_err(give_up)?;
    if let Some(dir) = s.original.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Copy every extended attribute `from` has onto `to`, each as far as allowed.
#[cfg(unix)]
fn copy_xattrs(from: &Path, to: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let (Ok(f), Ok(t)) = (std::ffi::CString::new(from.as_os_str().as_bytes()), std::ffi::CString::new(to.as_os_str().as_bytes())) else {
        return;
    };
    // SAFETY: NUL-terminated paths, and buffers of the lengths passed.
    unsafe {
        let len = xattr::list(f.as_ptr(), std::ptr::null_mut(), 0);
        if len <= 0 {
            return;
        }
        let mut names = vec![0u8; len as usize];
        let len = xattr::list(f.as_ptr(), names.as_mut_ptr().cast(), names.len());
        if len <= 0 {
            return;
        }
        for name in names[..len as usize].split(|&b| b == 0).filter(|n| !n.is_empty()) {
            let Ok(n) = std::ffi::CString::new(name) else { continue };
            let size = xattr::get(f.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0);
            if size < 0 {
                continue;
            }
            let mut value = vec![0u8; size as usize];
            let size = xattr::get(f.as_ptr(), n.as_ptr(), value.as_mut_ptr().cast(), value.len());
            if size >= 0 {
                xattr::set(t.as_ptr(), n.as_ptr(), value.as_ptr().cast(), size as usize);
            }
        }
    }
}

/// The extended-attribute calls. macOS's take two arguments more (a resource fork offset and
/// options); 0 for both is the Linux behaviour: whole values, following a symbolic link.
#[cfg(unix)]
mod xattr {
    use std::ffi::{c_char, c_void};

    /// The names `path` has, NUL-separated, into `buf` (`size` 0: the length needed).
    ///
    /// # Safety
    /// A NUL-terminated path and a buffer of `size` bytes (or null with 0).
    pub(super) unsafe fn list(path: *const c_char, buf: *mut c_char, size: usize) -> isize {
        #[cfg(target_os = "macos")]
        // SAFETY: as the caller ensures.
        return unsafe { libc::listxattr(path, buf, size, 0) };
        #[cfg(not(target_os = "macos"))]
        // SAFETY: as the caller ensures.
        return unsafe { libc::listxattr(path, buf, size) };
    }

    /// The value of `name` on `path`, into `value` (`size` 0: the length needed).
    ///
    /// # Safety
    /// NUL-terminated strings and a buffer of `size` bytes (or null with 0).
    pub(super) unsafe fn get(path: *const c_char, name: *const c_char, value: *mut c_void, size: usize) -> isize {
        #[cfg(target_os = "macos")]
        // SAFETY: as the caller ensures.
        return unsafe { libc::getxattr(path, name, value, size, 0, 0) };
        #[cfg(not(target_os = "macos"))]
        // SAFETY: as the caller ensures.
        return unsafe { libc::getxattr(path, name, value, size) };
    }

    /// Set `name` on `path` to the `size` bytes at `value`.
    ///
    /// # Safety
    /// NUL-terminated strings and a value of `size` bytes.
    pub(super) unsafe fn set(path: *const c_char, name: *const c_char, value: *const c_void, size: usize) -> i32 {
        #[cfg(target_os = "macos")]
        // SAFETY: as the caller ensures.
        return unsafe { libc::setxattr(path, name, value, size, 0, 0) };
        #[cfg(not(target_os = "macos"))]
        // SAFETY: as the caller ensures.
        return unsafe { libc::setxattr(path, name, value, size, 0) };
    }
}

#[cfg(unix)]
thread_local! {
    /// Set by `edit::hold` (tests): a replace on this thread is refused, as the system can
    /// refuse one (a folder that does not allow it).
    pub(crate) static REFUSE_REPLACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(not(any(windows, unix)))]
fn replace(s: &Staged) -> Result<(), Failure> {
    std::fs::rename(&s.temp, &s.original).map_err(|e| failure(&e))
}

/// The swap as two renames: the original to its backup name, the checked copy to the
/// original's name. Between the two the name is empty; a crash there is what `recover` mends
/// (the backup goes back). If the second rename fails, the original is put back at once.
#[cfg_attr(not(windows), allow(dead_code))]
fn swap_by_renames(s: &Staged) -> Result<(), Failure> {
    std::fs::rename(&s.original, &s.backup).map_err(|e| failure(&e))?;
    if let Err(e) = std::fs::rename(&s.temp, &s.original) {
        if let Err(back) = std::fs::rename(&s.backup, &s.original) {
            log::error!("edit: {} is at {} and could not be moved back: {back}", s.original.display(), s.backup.display());
            return Err(Failure::Other(format!("the original is safe as {} ({back})", s.backup.display())));
        }
        let _ = std::fs::remove_file(&s.temp);
        let _ = std::fs::remove_file(&s.record);
        return Err(failure(&e));
    }
    Ok(())
}

/// After a crash: finish or undo every write that was under way (records in `inflight`).
/// Returns what was done, for the log. A record whose folder is not there now (a drive not
/// connected, a share not reachable) is kept for the next start: its file may be at its
/// backup name, and only the record knows.
pub fn recover(inflight: &Path) -> Vec<String> {
    let mut done = Vec::new();
    let Ok(dir) = std::fs::read_dir(inflight) else { return done };
    for entry in dir.flatten() {
        let record = entry.path();
        if record.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(s) = std::fs::read(&record).ok().and_then(|b| serde_json::from_slice::<Staged>(&b).ok()) else {
            let _ = std::fs::remove_file(&record);
            continue;
        };
        // Only ever the editor's own names, beside the file the record names.
        let ours = |p: &Path| p.parent() == s.original.parent() && p.file_name().is_some_and(|n| n.to_string_lossy().contains(TEMP_MARK));
        if !ours(&s.temp) || !ours(&s.backup) {
            let _ = std::fs::remove_file(&record);
            continue;
        }
        if !s.original.parent().is_some_and(Path::is_dir) {
            done.push(format!("{} is not reachable now; its interrupted edit is mended at a later start", s.original.display()));
            continue;
        }
        if !s.original.exists() && s.backup.exists() {
            match std::fs::rename(&s.backup, &s.original) {
                Ok(()) => done.push(format!("restored {} from its backup", s.original.display())),
                Err(e) => {
                    done.push(format!("could not restore {} from {}: {e}", s.original.display(), s.backup.display()));
                    continue; // keep the record: try again next time
                }
            }
        }
        for p in [&s.temp, &s.backup] {
            if p.exists() && s.original.exists() {
                match std::fs::remove_file(p) {
                    Ok(()) => done.push(format!("removed {}", p.display())),
                    Err(e) => done.push(format!("could not remove {}: {e}", p.display())),
                }
            }
        }
        if !s.temp.exists() && !s.backup.exists() {
            let _ = std::fs::remove_file(&record);
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn io_errors_read_as_people_say_them() {
        let os = |n: i32| failure(&std::io::Error::from_raw_os_error(n));
        assert_eq!(os(112), Failure::DiskFull, "ERROR_DISK_FULL");
        assert_eq!(os(39), Failure::DiskFull, "ERROR_HANDLE_DISK_FULL");
        assert_eq!(os(32), Failure::InUse, "ERROR_SHARING_VIOLATION");
        assert_eq!(os(1175), Failure::InUse, "ERROR_UNABLE_TO_REMOVE_REPLACED");
        assert_eq!(os(5), Failure::Denied);
        assert_eq!(os(53), Failure::Unavailable, "ERROR_BAD_NETPATH");
        assert_eq!(os(21), Failure::Unavailable, "ERROR_NOT_READY: a drive gone");
        assert_eq!(os(2), Failure::Missing);
        assert!(Failure::DiskFull.to_string().contains("disk is full"));
    }

    #[cfg(unix)]
    #[test]
    fn io_errors_read_as_people_say_them() {
        let os = |n: i32| failure(&std::io::Error::from_raw_os_error(n));
        assert_eq!(os(libc::ENOSPC), Failure::DiskFull);
        assert_eq!(os(libc::EDQUOT), Failure::DiskFull, "over quota");
        assert_eq!(os(libc::EBUSY), Failure::InUse);
        assert_eq!(os(libc::EACCES), Failure::Denied);
        assert_eq!(os(libc::EROFS), Failure::Denied, "a read-only filesystem");
        assert_eq!(os(libc::ENOTCONN), Failure::Unavailable, "a network mount gone");
        assert_eq!(os(libc::EIO), Failure::Unavailable, "a drive pulled out");
        assert_eq!(os(libc::ENOENT), Failure::Missing);
    }

    /// The copy takes the original's place with its permission bits and extended
    /// attributes, and a reader of the old file reads on from it.
    #[cfg(unix)]
    #[test]
    fn a_replace_keeps_mode_and_attributes_and_spares_an_open_reader() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testdir::TestDir::new("edit-attrs");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("01 Song.flac");
        std::fs::write(&file, b"old content").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: NUL-terminated strings and a value of the length given.
        let tagged = unsafe { xattr::set(c(&file).as_ptr(), c"user.mp-test".as_ptr(), b"kept".as_ptr().cast(), 4) } == 0;
        let mut reader = std::fs::File::open(&file).unwrap();
        let s = stage(&file, &dir.join("inflight"), 0).unwrap();
        std::fs::write(&s.temp, b"new content").unwrap();
        commit(&s).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new content");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o640);
        if tagged {
            let mut v = [0u8; 8];
            // SAFETY: as above, into a local buffer of the length given.
            let n = unsafe { xattr::get(c(&file).as_ptr(), c"user.mp-test".as_ptr(), v.as_mut_ptr().cast(), v.len()) };
            assert_eq!(&v[..n.max(0) as usize], b"kept");
        }
        let mut old = Vec::new();
        reader.read_to_end(&mut old).unwrap();
        assert_eq!(old, b"old content", "an open file reads on from the old data");
        assert!(!s.temp.exists() && !s.backup.exists() && !s.record.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporary_names_sit_beside_the_file_and_are_recognised() {
        let p = Path::new(r"M:\Music\A\01 Song.flac");
        let t = sibling(p, "abcd1234", "tmp");
        assert_eq!(t, Path::new(r"M:\Music\A\01 Song.flac.mpedit-abcd1234.tmp"));
        assert!(!crate::scan::is_audio(&t), "the scanner never takes one for music");
        assert!(t.to_string_lossy().contains(TEMP_MARK));
        assert_ne!(token(), token());
    }

    #[test]
    fn a_swap_by_renames_is_whole_or_untouched_and_recovery_waits_for_an_absent_drive() {
        let dir = crate::testdir::TestDir::new("edit-renames");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inflight")).unwrap();
        let original = dir.join("song.flac");
        let staged = |t: &str| Staged {
            original: original.clone(),
            temp: sibling(&original, t, "tmp"),
            backup: sibling(&original, t, "bak"),
            record: dir.join("inflight").join(format!("{t}.json")),
        };
        // The swap (what a volume without ReplaceFileW gets): the new file in place, nothing else.
        std::fs::write(&original, b"old").unwrap();
        let s = staged("a1");
        std::fs::write(&s.temp, b"new").unwrap();
        swap_by_renames(&s).unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), b"new");
        let _ = std::fs::remove_file(&s.backup);
        // Interrupted between the renames: the next start puts the old file back.
        std::fs::write(&original, b"old").unwrap();
        let s = staged("b2");
        std::fs::write(&s.record, serde_json::to_vec(&s).unwrap()).unwrap();
        std::fs::write(&s.temp, b"new").unwrap();
        std::fs::rename(&original, &s.backup).unwrap();
        recover(&dir.join("inflight"));
        assert_eq!(std::fs::read(&original).unwrap(), b"old");
        assert!(!s.temp.exists() && !s.backup.exists() && !s.record.exists());
        // The same with the drive not connected at the next start: the record waits for it.
        let away = Staged {
            original: PathBuf::from(r"Q:\not connected\song.flac"),
            temp: PathBuf::from(r"Q:\not connected\song.flac.mpedit-c3.tmp"),
            backup: PathBuf::from(r"Q:\not connected\song.flac.mpedit-c3.bak"),
            record: dir.join("inflight").join("c3.json"),
        };
        std::fs::write(&away.record, serde_json::to_vec(&away).unwrap()).unwrap();
        let said = recover(&dir.join("inflight"));
        assert!(away.record.exists(), "kept for a later start: {said:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A swap that failed with the original at its backup name, not moved back (the drive went
    /// mid-call, another program took the name): giving the write up (`save_file` does on any
    /// error) keeps the record, so the next start puts the original back under its name.
    #[test]
    fn giving_up_a_swap_that_left_the_original_at_its_backup_name_keeps_the_record() {
        let dir = crate::testdir::TestDir::new("edit-abandon");
        std::fs::create_dir_all(dir.join("inflight")).unwrap();
        let original = dir.join("song.flac");
        let staged = |t: &str| Staged {
            original: original.clone(),
            temp: sibling(&original, t, "tmp"),
            backup: sibling(&original, t, "bak"),
            record: dir.join("inflight").join(format!("{t}.json")),
        };
        let s = staged("d4");
        std::fs::write(&s.record, serde_json::to_vec(&s).unwrap()).unwrap();
        std::fs::write(&s.temp, b"new").unwrap();
        std::fs::write(&s.backup, b"old").unwrap();
        abandon(&s);
        assert!(s.record.exists(), "only the record knows where the original is");
        recover(&dir.join("inflight"));
        assert_eq!(std::fs::read(&original).unwrap(), b"old");
        assert!(!s.temp.exists() && !s.backup.exists() && !s.record.exists());
        // Given up with the original in its place: the copy and the record go, as before.
        let s = staged("e5");
        std::fs::write(&s.record, serde_json::to_vec(&s).unwrap()).unwrap();
        std::fs::write(&s.temp, b"new").unwrap();
        abandon(&s);
        assert!(!s.temp.exists() && !s.record.exists());
        assert_eq!(std::fs::read(&original).unwrap(), b"old");
    }

    #[test]
    fn recovery_ignores_records_that_name_files_not_its_own() {
        let dir = crate::testdir::TestDir::new("edit-recover");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inflight")).unwrap();
        let victim = dir.join("keep me.flac");
        std::fs::write(&victim, b"music").unwrap();
        // A record (made up, or damaged) naming a file that is not the editor's.
        let s = Staged { original: dir.join("x.flac"), temp: victim.clone(), backup: dir.join("x.flac.mpedit-1.bak"), record: PathBuf::new() };
        std::fs::write(dir.join("inflight").join("bad.json"), serde_json::to_vec(&s).unwrap()).unwrap();
        recover(&dir.join("inflight"));
        assert!(victim.exists(), "never removed");
        assert_eq!(std::fs::read_dir(dir.join("inflight")).unwrap().count(), 0, "the record goes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
