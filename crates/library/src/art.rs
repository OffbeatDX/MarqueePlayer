//! Album art cache: downscaled copies on disk, keyed by a hash of the source image.
//!
//! Each source image becomes four files in the cache directory:
//! - `<stem>_t.jpg`: grid thumbnail, longest side `THUMB_PX`.
//! - `<stem>_f.jpg`: album page / now playing, longest side `FULL_PX`.
//! - `<stem>_a.txt`: the art's accent colour (`rrggbb`, or `none`), see `accent.rs`.
//! - `<stem>_b.jpg`: the pre-blurred backdrop. Small and heavily blurred: it is shown
//!   stretched across the whole window, where bilinear upscaling of a blurred image is
//!   indistinguishable from blurring at full size.
//!
//! The accent file's second line is the cache format (`v2`) the images were made in.
//! When the format changes (sizes or JPEG quality), `version.txt` in the cache folder
//! is behind and the next scan re-caches every album's art from its source once
//! (`needs_upgrade`, `is_current`, scan.rs), replacing the files in place: nothing is
//! deleted, and the old images show until the new ones are written.
//!
//! Covers no album refers to any more (`albums.art`) are removed by `sweep`: only files
//! of the cache's own names in this folder, and only those untouched for `SWEEP_GRACE`
//! (`store` touches a cover it finds cached, so one about to be referred to again is
//! never taken for unused).
//!
//! The music folders are never written to.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::DynamicImage;
#[cfg(test)]
use image::ImageFormat;

pub const THUMB_PX: u32 = 360;
/// The album page cover and now playing (up to 620 px wide on screen: ~1240 pixels at
/// 200% scaling).
pub const FULL_PX: u32 = 1200;
const BLUR_PX: u32 = 192;
const BLUR_SIGMA: f32 = 9.0;
/// JPEG quality. The encoder's default (75) leaves visible blocking in flat colour and
/// ringing at type edges on a large cover; 88 does not show it, at ~1.5x the bytes.
/// Thumbnails are small on screen: 85. The backdrop is a blur: 75 is plenty.
const FULL_QUALITY: u8 = 88;
const THUMB_QUALITY: u8 = 85;
const BLUR_QUALITY: u8 = 75;
/// The format above. Bump it when a size or a quality changes: caches made by older
/// formats are then re-made once (see the module doc).
pub const CACHE_VERSION: u32 = 2;

/// Folder images tried in order before falling back to any image in the folder.
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "albumart", "album"];
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];
/// How long a cache file no album refers to is kept anyway: whatever stored it (a scan's
/// batch, a server cover) may not have written the album yet.
pub const SWEEP_GRACE: Duration = Duration::from_secs(60 * 60);

/// What `sweep` removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub files: usize,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct ArtCache {
    dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtPaths {
    pub thumb: PathBuf,
    pub full: PathBuf,
    pub blur: PathBuf,
}

/// FNV-1a, 64 bit. Stable across builds (std's hasher is not), which matters because
/// the result names files on disk.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

impl ArtCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let _ = std::fs::create_dir_all(&dir);
        Self { dir }
    }

    pub fn paths(&self, stem: &str) -> ArtPaths {
        ArtPaths {
            thumb: self.dir.join(format!("{stem}_t.jpg")),
            full: self.dir.join(format!("{stem}_f.jpg")),
            blur: self.dir.join(format!("{stem}_b.jpg")),
        }
    }

    /// Cache an image (encoded bytes) and return its stem. Cheap when already cached in
    /// the current format.
    pub fn store(&self, bytes: &[u8]) -> Result<String, String> {
        let stem = format!("{:016x}", fnv1a(bytes));
        if self.is_current(&stem) {
            // In use again: not for `sweep` to take, although no album may say so yet.
            self.touch(&stem);
            return Ok(stem);
        }
        let p = self.paths(&stem);
        let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
        let img = DynamicImage::ImageRgb8(img.to_rgb8());
        save(&fit(&img, FULL_PX), &p.full, FULL_QUALITY)?;
        save(&fit(&img, THUMB_PX), &p.thumb, THUMB_QUALITY)?;
        let small = img.resize_exact(BLUR_PX, BLUR_PX, FilterType::Triangle);
        save(&small.blur(BLUR_SIGMA), &p.blur, BLUR_QUALITY)?;
        let _ = self.write_accent(&stem, &img);
        Ok(stem)
    }

    /// Whether this art is cached, in the current format.
    pub fn is_current(&self, stem: &str) -> bool {
        let p = self.paths(stem);
        p.thumb.exists()
            && p.full.exists()
            && p.blur.exists()
            && std::fs::read_to_string(self.accent_path(stem))
                .is_ok_and(|t| t.lines().nth(1).map(str::trim) == Some(format_tag().as_str()))
    }

    /// Take this art as current although it was not re-made (its source is gone): the
    /// upgrade does not try it again.
    pub fn mark_current(&self, stem: &str) {
        if self.is_current(stem) {
            return;
        }
        let colour = std::fs::read_to_string(self.accent_path(stem))
            .ok()
            .and_then(|t| t.lines().next().map(|l| l.trim().to_owned()))
            .filter(|l| !l.is_empty());
        match colour {
            Some(c) => {
                let _ = std::fs::write(self.accent_path(stem), format!("{c}\n{}\n", format_tag()));
            }
            None => {
                if let Ok(img) = image::open(self.paths(stem).thumb) {
                    self.write_accent(stem, &img);
                }
            }
        }
    }

    fn version_path(&self) -> PathBuf {
        self.dir.join("version.txt")
    }

    /// The cache was made (at least partly) by an older format: the next scan re-makes
    /// what is not current, then calls `mark_upgraded`.
    pub fn needs_upgrade(&self) -> bool {
        std::fs::read_to_string(self.version_path()).ok().and_then(|t| t.trim().parse::<u32>().ok()) != Some(CACHE_VERSION)
    }

    pub fn mark_upgraded(&self) {
        let _ = std::fs::write(self.version_path(), format!("{CACHE_VERSION}\n"));
    }

    fn accent_path(&self, stem: &str) -> PathBuf {
        self.dir.join(format!("{stem}_a.txt"))
    }

    /// The accent file: the colour (`rrggbb` or `none`), then the cache format.
    fn write_accent(&self, stem: &str, img: &DynamicImage) -> Option<[u8; 3]> {
        let small = img.resize_exact(64, 64, FilterType::Triangle).to_rgb8();
        let colour = crate::accent::extract(small.pixels().map(|p| p.0));
        let text = colour.map_or_else(|| "none".to_owned(), |[r, g, b]| format!("{r:02x}{g:02x}{b:02x}"));
        let _ = std::fs::write(self.accent_path(stem), format!("{text}\n{}\n", format_tag()));
        colour
    }

    /// The art's accent colour as found in the art (unclamped; see `accent::usable`).
    /// Read from the cache, or computed from the thumbnail and cached for art stored
    /// before accents existed. Does file IO and decoding: call it off the UI thread.
    pub fn accent(&self, stem: &str) -> Option<[u8; 3]> {
        if let Ok(text) = std::fs::read_to_string(self.accent_path(stem)) {
            let t = text.lines().next().unwrap_or("").trim();
            if t == "none" {
                return None;
            }
            if let Ok(v) = u32::from_str_radix(t, 16) {
                return Some([(v >> 16) as u8, (v >> 8) as u8, v as u8]);
            }
        }
        let img = image::open(self.paths(stem).thumb).ok()?;
        self.write_accent(stem, &img)
    }

    /// Up to three of the art's colours, most prominent first (`accent::palette`), from the
    /// thumbnail. Not cached: a 360 px decode and a 64 x 64 vote, a few milliseconds. Does
    /// file IO and decoding: call it off the UI thread.
    pub fn palette(&self, stem: &str) -> Vec<[u8; 3]> {
        let Ok(img) = image::open(self.paths(stem).thumb) else { return Vec::new() };
        let small = img.resize_exact(64, 64, FilterType::Triangle).to_rgb8();
        crate::accent::palette(small.pixels().map(|p| p.0), 3)
    }

    pub fn store_file(&self, path: &Path) -> Result<String, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        self.store(&bytes)
    }

    /// Mark the stem as just used (its accent file's time), for `sweep`.
    fn touch(&self, stem: &str) {
        if let Ok(f) = std::fs::File::options().write(true).open(self.accent_path(stem)) {
            let _ = f.set_modified(SystemTime::now());
        }
    }

    /// Whether any file of `stem` changed less than `grace` before `now`.
    fn recent(&self, stem: &str, now: SystemTime, grace: Duration) -> bool {
        let p = self.paths(stem);
        [p.thumb, p.full, p.blur, self.accent_path(stem)]
            .iter()
            .filter_map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
            .any(|t| now.duration_since(t).map_or(true, |age| age < grace))
    }

    /// Remove the cached covers whose stem is not in `in_use` (asked for after the folder
    /// is listed, so a cover stored meanwhile is either in it or recent), and leftovers of
    /// a write cut short. Only this folder's own files (`<stem>_t.jpg`, `_f.jpg`, `_b.jpg`,
    /// `_a.txt` and their temporary names; never a folder or a link), and only for a stem
    /// none of whose files changed in the last `grace` (`SWEEP_GRACE`), looked at again
    /// just before its files go. Nothing is removed when `in_use` fails. Does file IO: off
    /// the UI thread.
    pub fn sweep(&self, grace: Duration, in_use: impl FnOnce() -> Result<HashSet<String>, String>) -> Result<Swept, String> {
        let rd = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Swept::default()),
            Err(e) => return Err(e.to_string()),
        };
        let mut by_stem: HashMap<String, Vec<(PathBuf, u64)>> = HashMap::new();
        for entry in rd.flatten() {
            // `file_type` does not follow links: plain files only.
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let name = entry.file_name();
            let Some(stem) = name.to_str().and_then(cache_stem) else { continue };
            let len = entry.metadata().map_or(0, |m| m.len());
            by_stem.entry(stem.to_owned()).or_default().push((entry.path(), len));
        }
        if by_stem.is_empty() {
            return Ok(Swept::default());
        }
        let in_use = in_use()?;
        let mut swept = Swept::default();
        for (stem, files) in by_stem {
            if in_use.contains(&stem) || self.recent(&stem, SystemTime::now(), grace) {
                continue;
            }
            // A leftover's own time counts too (it is no file of `paths`).
            for (path, len) in files {
                let young = std::fs::metadata(&path).and_then(|m| m.modified()).is_ok_and(|t| SystemTime::now().duration_since(t).map_or(true, |age| age < grace));
                if !young && std::fs::remove_file(&path).is_ok() {
                    swept.files += 1;
                    swept.bytes += len;
                }
            }
        }
        Ok(swept)
    }
}

/// The stem of a file the cache writes: `<16 hex>_<t|f|b>.jpg`, `<16 hex>_a.txt`, or a
/// temporary name of one (`<16 hex>_<t|f|b>.<pid>-<n>.tmp`). Nothing else is the cache's.
fn cache_stem(name: &str) -> Option<&str> {
    let (stem, rest) = (name.get(..16)?, name.get(16..)?);
    if !stem.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
        return None;
    }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    let ok = match rest {
        "_t.jpg" | "_f.jpg" | "_b.jpg" | "_a.txt" => true,
        _ => ["_t.", "_f.", "_b."]
            .iter()
            .find_map(|p| rest.strip_prefix(p))
            .and_then(|r| r.strip_suffix(".tmp"))
            .and_then(|r| r.split_once('-'))
            .is_some_and(|(pid, n)| digits(pid) && digits(n)),
    };
    ok.then_some(stem)
}

/// The stems the library's albums use (`albums.art`): what `ArtCache::sweep` keeps.
pub fn stems_in_use(conn: &rusqlite::Connection) -> rusqlite::Result<HashSet<String>> {
    let mut st = conn.prepare("SELECT DISTINCT art FROM albums WHERE art IS NOT NULL")?;
    st.query_map([], |r| r.get::<_, String>(0))?.collect()
}

/// `ArtCache::sweep` against the library in `conn`.
pub fn sweep_unused(conn: &rusqlite::Connection, cache: &ArtCache) -> Result<Swept, String> {
    cache.sweep(SWEEP_GRACE, || stems_in_use(conn).map_err(|e| e.to_string()))
}

/// Shrink so the longest side is at most `px`. Never enlarges.
fn fit(img: &DynamicImage, px: u32) -> DynamicImage {
    if img.width().max(img.height()) <= px {
        return img.clone();
    }
    img.resize(px, px, FilterType::Lanczos3)
}

/// The cache format as written in accent files.
fn format_tag() -> String {
    format!("v{CACHE_VERSION}")
}

fn save(img: &DynamicImage, path: &Path, quality: u8) -> Result<(), String> {
    // Write to a temp name and rename, so a crash never leaves a half-written file
    // that `store` would later treat as cached.
    // The temp name is unique per call: albums sharing one cover are cached
    // concurrently by different scan threads.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("{}-{n}.tmp", std::process::id()));
    let file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    if let Err(e) = write_jpeg(img, file, quality) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        // Another thread won the race with identical content.
        Err(_) if path.exists() => {
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Encode `img` as a JPEG into `out`, every byte of it written: the buffer's last flush is
/// checked, not left to the drop (a full disk shows there, and a file cut short must never be
/// renamed into place, where `store` would take it for cached for good).
fn write_jpeg<W: std::io::Write>(img: &DynamicImage, out: W, quality: u8) -> Result<W, String> {
    let mut buf = std::io::BufWriter::new(out);
    img.write_with_encoder(JpegEncoder::new_with_quality(&mut buf, quality)).map_err(|e| e.to_string())?;
    buf.into_inner().map_err(|e| e.error().to_string())
}

/// Whether a path names an image the art cache can take (by extension).
pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The cover image file in an album folder, if any.
pub fn find_folder_cover(folder: &Path) -> Option<PathBuf> {
    let mut images: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| IMAGE_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        })
        .collect();
    images.sort();
    let stem_of = |p: &PathBuf| p.file_stem().and_then(|s| s.to_str()).map(str::to_ascii_lowercase);
    for name in COVER_NAMES {
        if let Some(p) = images.iter().find(|p| stem_of(p).as_deref() == Some(name)) {
            return Some(p.clone());
        }
    }
    images.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_cache_s_own_names_are_swept() {
        assert_eq!(cache_stem("0123456789abcdef_t.jpg"), Some("0123456789abcdef"));
        assert_eq!(cache_stem("0123456789abcdef_a.txt"), Some("0123456789abcdef"));
        assert_eq!(cache_stem("0123456789abcdef_f.1234-7.tmp"), Some("0123456789abcdef"));
        for other in [
            "version.txt",
            "0123456789ABCDEF_t.jpg",
            "0123456789abcdef_x.jpg",
            "0123456789abcde_t.jpg",
            "0123456789abcdef_t.jpg.bak",
            "0123456789abcdef_a.1-2.tmp",
            "0123456789abcdef_t.-2.tmp",
            "cover.jpg",
            "0123456789abcdef",
        ] {
            assert_eq!(cache_stem(other), None, "{other}");
        }
    }

    /// Unused covers go, used ones and other files stay; a cover stored again (found
    /// cached) is not taken for unused; nothing recent goes.
    #[test]
    fn sweep_removes_only_covers_no_album_uses() {
        let dir = crate::testdir::TestDir::new("art-sweep");
        let _ = std::fs::remove_dir_all(&dir);
        let cache = ArtCache::new(&dir);
        let png = |c: u8| {
            let mut bytes = Vec::new();
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(40, 40, image::Rgb([c, 10, 200])))
                .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
                .unwrap();
            bytes
        };
        let (kept, gone, again) = (cache.store(&png(1)).unwrap(), cache.store(&png(2)).unwrap(), cache.store(&png(3)).unwrap());
        cache.mark_upgraded();
        std::fs::write(dir.join("notes.txt"), b"not the cache's").unwrap();
        let leftover = dir.join(format!("{gone}_t.99-1.tmp"));
        std::fs::write(&leftover, b"cut short").unwrap();
        std::fs::create_dir_all(dir.join("0123456789abcdef_t.jpg")).unwrap();
        let used = || Ok::<_, String>(HashSet::from([kept.clone()]));
        // Everything is new: nothing goes within the grace period.
        assert_eq!(cache.sweep(SWEEP_GRACE, used).unwrap(), Swept::default());
        // A short grace: `again`, found cached (and so touched) just now, stays.
        let grace = Duration::from_millis(400);
        std::thread::sleep(grace + Duration::from_millis(100));
        assert_eq!(cache.store(&png(3)).unwrap(), again, "found cached");
        let swept = cache.sweep(grace, used).unwrap();
        assert_eq!(swept.files, 5, "the four files of the unused cover and its leftover: {swept:?}");
        assert!(swept.bytes > 0);
        assert!(cache.is_current(&kept) && cache.is_current(&again) && !cache.is_current(&gone));
        assert!(!leftover.exists() && !cache.paths(&gone).thumb.exists());
        assert!(dir.join("notes.txt").exists() && dir.join("version.txt").exists() && dir.join("0123456789abcdef_t.jpg").is_dir(), "not the cache's own");
        // Saying what is in use fails: nothing goes.
        assert!(cache.sweep(Duration::ZERO, || Err("no library".to_owned())).is_err());
        assert!(cache.is_current(&again));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn stores_three_sizes_once() {
        let dir = crate::testdir::TestDir::new("art-test");
        let cache = ArtCache::new(&dir);
        let img = image::RgbImage::from_fn(1200, 1000, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let stem = cache.store(&bytes).unwrap();
        let p = cache.paths(&stem);
        let t = image::open(&p.thumb).unwrap();
        assert_eq!((t.width(), t.height()), (360, 300));
        assert_eq!(image::open(&p.blur).unwrap().width(), BLUR_PX);
        assert!(dir.join(format!("{stem}_a.txt")).exists());
        assert!(cache.is_current(&stem));
        assert_eq!(cache.store(&bytes).unwrap(), stem);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A disk that fails the last write (full): the file is not taken as written. A small image
    /// is all in the buffer until its final flush, which a drop would ignore.
    #[test]
    fn a_write_that_fails_at_the_end_is_an_error() {
        struct Full;
        impl std::io::Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(16, 16, |x, y| image::Rgb([x as u8 * 16, y as u8 * 16, 0])));
        assert_eq!(write_jpeg(&img, Full, 90).err().as_deref(), Some("disk full"));
        let bytes = write_jpeg(&img, Vec::new(), 90).unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 16, "and a write that works is whole");
    }

    /// Art cached by the previous format (900 px, default quality, an accent file with
    /// only the colour) is not current; storing it again re-makes it in place, and the
    /// accent colour reads the same either way.
    #[test]
    fn an_older_format_is_remade_in_place() {
        let dir = crate::testdir::TestDir::new("art-upgrade");
        let _ = std::fs::remove_dir_all(&dir);
        let cache = ArtCache::new(&dir);
        assert!(cache.needs_upgrade(), "a cache without a version file");
        let img = image::RgbImage::from_fn(1600, 1600, |x, y| image::Rgb([(x / 7 % 256) as u8, (y / 5 % 256) as u8, 200]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png).unwrap();
        let stem = cache.store(&bytes).unwrap();
        let p = cache.paths(&stem);
        // Pretend it was made by the old format.
        let old = image::open(&p.full).unwrap().resize(900, 900, FilterType::Triangle);
        old.save(&p.full).unwrap();
        let colour = cache.accent(&stem);
        let first = std::fs::read_to_string(dir.join(format!("{stem}_a.txt"))).unwrap().lines().next().unwrap().to_owned();
        std::fs::write(dir.join(format!("{stem}_a.txt")), &first).unwrap();
        assert!(!cache.is_current(&stem));
        assert_eq!(cache.accent(&stem), colour, "an old accent file still reads");
        assert_eq!(cache.store(&bytes).unwrap(), stem);
        assert!(cache.is_current(&stem));
        assert_eq!(image::open(&p.full).unwrap().width(), FULL_PX, "re-made at the new size");
        cache.mark_upgraded();
        assert!(!cache.needs_upgrade());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
