//! Gapless information for AAC in MP4 (`.m4a`, `.m4b`, `.mp4`), which Symphonia 0.6 does
//! not read: the encoder delay (priming) and the real length, from the audio track's
//! edit list (`moov/trak/edts/elst`, as ffmpeg and most encoders write it), else from
//! iTunes' `iTunSMPB` comment (`moov/udta/meta/ilst/----`). Without either, nothing is
//! trimmed, as before.
//!
//! Only the box headers are read (seeking past `mdat`), so this costs a few small reads.
//! A file still being written is read through its feed (`read_from` over a `FeedReader`),
//! never directly: its `moov` box may not be on disk yet.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// (delay, frames): drop the first `delay` decoded frames, play `frames` after them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trim {
    pub delay: u64,
    pub frames: u64,
    /// The track's timescale (units of `delay` and `frames`): usually its sample rate.
    pub scale: u64,
}

struct Reader<R> {
    r: R,
}

impl<R: Read + Seek> Reader<R> {
    /// Boxes in [start, end): (type, payload start, payload end).
    fn children(&mut self, start: u64, end: u64) -> Vec<([u8; 4], u64, u64)> {
        let mut out = Vec::new();
        let mut at = start;
        while at + 8 <= end {
            if self.r.seek(SeekFrom::Start(at)).is_err() {
                break;
            }
            let mut h = [0u8; 8];
            if self.r.read_exact(&mut h).is_err() {
                break;
            }
            let mut size = u64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]]));
            let kind = [h[4], h[5], h[6], h[7]];
            let mut header = 8;
            if size == 1 {
                let mut l = [0u8; 8];
                if self.r.read_exact(&mut l).is_err() {
                    break;
                }
                size = u64::from_be_bytes(l);
                header = 16;
            } else if size == 0 {
                size = end - at;
            }
            // A size past the end, or one that wraps around (a damaged or crafted file).
            let Some(box_end) = at.checked_add(size).filter(|&e| size >= header && e <= end) else {
                break;
            };
            out.push((kind, at + header, box_end));
            at = box_end;
        }
        out
    }

    fn bytes(&mut self, start: u64, end: u64) -> Option<Vec<u8>> {
        let len = end.checked_sub(start)?;
        if len > 1 << 20 {
            return None;
        }
        self.r.seek(SeekFrom::Start(start)).ok()?;
        let mut v = vec![0; len as usize];
        self.r.read_exact(&mut v).ok()?;
        Some(v)
    }
}

fn be32(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 4).map(|x| u64::from(u32::from_be_bytes([x[0], x[1], x[2], x[3]])))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8).map(|x| u64::from_be_bytes(x.try_into().unwrap()))
}

/// A full box's (version, timescale, duration) from `mvhd` or `mdhd`.
fn header_times(b: &[u8]) -> Option<(u64, u64)> {
    match b.first()? {
        1 => Some((be32(b, 20)?, be64(b, 24)?)),
        _ => Some((be32(b, 12)?, be32(b, 16)?)),
    }
}

pub fn read(path: &Path) -> Option<Trim> {
    let f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    read_from(f, len)
}

/// `read` over any reader of the whole file, `len` bytes long.
pub fn read_from<R: Read + Seek>(r: R, len: u64) -> Option<Trim> {
    let mut rd = Reader { r: std::io::BufReader::new(r) };
    let top = rd.children(0, len);
    let &(_, ms, me) = top.iter().find(|b| &b.0 == b"moov")?;
    let moov = rd.children(ms, me);
    let movie_scale = moov.iter().find(|b| &b.0 == b"mvhd").and_then(|&(_, s, e)| rd.bytes(s, e)).and_then(|b| header_times(&b)).map(|t| t.0)?;
    for &(kind, ts, te) in &moov {
        if &kind != b"trak" {
            continue;
        }
        let trak = rd.children(ts, te);
        let Some(&(_, mds, mde)) = trak.iter().find(|b| &b.0 == b"mdia") else { continue };
        let mdia = rd.children(mds, mde);
        let is_audio = mdia.iter().find(|b| &b.0 == b"hdlr").and_then(|&(_, s, e)| rd.bytes(s, e)).is_some_and(|h| h.get(8..12) == Some(b"soun"));
        if !is_audio {
            continue;
        }
        let Some((media_scale, media_duration)) = mdia.iter().find(|b| &b.0 == b"mdhd").and_then(|&(_, s, e)| rd.bytes(s, e)).and_then(|b| header_times(&b)) else {
            continue;
        };
        if let Some(&(_, es, ee)) = trak.iter().find(|b| &b.0 == b"edts")
            && let Some(&(_, ls, le)) = rd.children(es, ee).iter().find(|b| &b.0 == b"elst")
            && let Some(b) = rd.bytes(ls, le)
        {
            let version = *b.first()?;
            let count = be32(&b, 4)?;
            // Skip empty edits (media_time -1); the first real one carries the trim.
            for i in 0..count as usize {
                let (dur, media_time) = if version == 1 {
                    (be64(&b, 8 + i * 20)?, be64(&b, 16 + i * 20)? as i64)
                } else {
                    (be32(&b, 8 + i * 12)?, i64::from(be32(&b, 12 + i * 12)? as u32 as i32))
                };
                if media_time < 0 {
                    continue;
                }
                // Segment duration is in the movie's timescale; convert to the track's.
                let frames = if movie_scale > 0 { (u128::from(dur) * u128::from(media_scale) / u128::from(movie_scale)) as u64 } else { 0 };
                let delay = media_time as u64;
                if frames > 0 && delay.checked_add(frames).is_some_and(|e| e <= media_duration.saturating_add(media_scale)) {
                    return Some(Trim { delay, frames, scale: media_scale });
                }
                break;
            }
        }
        return itunsmpb(&mut rd, &moov).map(|t| Trim { scale: media_scale, ..t });
    }
    None
}

/// iTunes: " 00000000 00000840 000001C0 0000000000A5CF00 ..." (delay, padding, frames).
fn itunsmpb<R: Read + Seek>(rd: &mut Reader<R>, moov: &[([u8; 4], u64, u64)]) -> Option<Trim> {
    let &(_, us, ue) = moov.iter().find(|b| &b.0 == b"udta")?;
    let &(_, ms, me) = rd.children(us, ue).iter().find(|b| &b.0 == b"meta")?;
    // `meta` is a full box: 4 bytes of version and flags before its children.
    let &(_, is, ie) = rd.children(ms + 4, me).iter().find(|b| &b.0 == b"ilst")?;
    for &(kind, s, e) in &rd.children(is, ie) {
        if &kind != b"----" {
            continue;
        }
        let parts = rd.children(s, e);
        let name = parts.iter().find(|b| &b.0 == b"name").and_then(|&(_, s, e)| rd.bytes(s, e))?;
        if name.get(4..) != Some(b"iTunSMPB") {
            continue;
        }
        let data = parts.iter().find(|b| &b.0 == b"data").and_then(|&(_, s, e)| rd.bytes(s, e))?;
        let text = String::from_utf8_lossy(data.get(8..)?).into_owned();
        let f: Vec<u64> = text.split_whitespace().filter_map(|x| u64::from_str_radix(x, 16).ok()).collect();
        if f.len() >= 4 && f[3] > 0 {
            return Some(Trim { delay: f[1], frames: f[3], scale: 0 });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    /// A version-0 mvhd / mdhd: flags, two times, timescale, duration.
    fn hd(kind: &[u8; 4], scale: u32, dur: u32) -> Vec<u8> {
        let mut p = vec![0u8; 12];
        p.extend(scale.to_be_bytes());
        p.extend(dur.to_be_bytes());
        p.extend([0u8; 8]);
        bx(kind, &p)
    }

    fn file(edts: Option<Vec<u8>>, udta: Option<Vec<u8>>) -> std::path::PathBuf {
        let mut hdlr = vec![0u8; 8];
        hdlr.extend(b"soun");
        hdlr.extend([0u8; 12]);
        let mdia = bx(b"mdia", &[hd(b"mdhd", 44_100, 266_240), bx(b"hdlr", &hdlr)].concat());
        let mut trak = Vec::new();
        if let Some(e) = edts {
            trak.extend(e);
        }
        trak.extend(mdia);
        let mut moov = hd(b"mvhd", 1000, 6000);
        moov.extend(bx(b"trak", &trak));
        if let Some(u) = udta {
            moov.extend(u);
        }
        let data = [bx(b"ftyp", b"M4A \0\0\0\0"), bx(b"mdat", &[0u8; 64]), bx(b"moov", &moov)].concat();
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("mp-mp4gapless-{}-{}.m4a", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::write(&p, data).unwrap();
        p
    }

    #[test]
    fn reads_the_edit_list() {
        // 6000 ms of movie time from media time 1024: delay 1024, 264600 frames at 44.1 kHz.
        let mut elst = vec![0u8, 0, 0, 0];
        elst.extend(1u32.to_be_bytes());
        elst.extend(6000u32.to_be_bytes());
        elst.extend(1024u32.to_be_bytes());
        elst.extend([0, 1, 0, 0]);
        let p = file(Some(bx(b"edts", &bx(b"elst", &elst))), None);
        let got = read(&p);
        let _ = std::fs::remove_file(p);
        assert_eq!(got, Some(Trim { delay: 1024, frames: 264_600, scale: 44_100 }));
    }

    #[test]
    fn reads_itunsmpb_without_an_edit_list() {
        let mut name = vec![0u8; 4];
        name.extend(b"iTunSMPB");
        let mut data = vec![0u8, 0, 0, 1, 0, 0, 0, 0];
        data.extend(b" 00000000 00000840 000001C0 0000000000040998".as_slice());
        let free = bx(b"----", &[bx(b"mean", b"\0\0\0\0com.apple.iTunes"), bx(b"name", &name), bx(b"data", &data)].concat());
        let mut meta = vec![0u8; 4];
        meta.extend(bx(b"ilst", &free));
        let p = file(None, Some(bx(b"udta", &bx(b"meta", &meta))));
        let got = read(&p);
        let _ = std::fs::remove_file(p);
        assert_eq!(got, Some(Trim { delay: 0x840, frames: 0x40998, scale: 44_100 }));
        let p = file(None, None);
        let got = read(&p);
        let _ = std::fs::remove_file(p);
        assert_eq!(got, None, "neither: nothing trimmed");
    }

    /// `read_from` on its own thread: `None` if it has not returned within a few seconds.
    fn read_bounded(data: Vec<u8>) -> Option<Option<Trim>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let len = data.len() as u64;
            let _ = tx.send(read_from(std::io::Cursor::new(data), len));
        });
        rx.recv_timeout(std::time::Duration::from_secs(5)).ok()
    }

    #[test]
    fn a_box_size_that_wraps_around_ends_the_walk() {
        let p = file(None, None);
        let mut data = std::fs::read(&p).unwrap();
        let _ = std::fs::remove_file(p);
        // After moov: a box whose 64-bit size takes the walk back to the start of the file.
        let at = data.len() as u64;
        data.extend(1u32.to_be_bytes());
        data.extend(b"free");
        data.extend(0u64.wrapping_sub(at).to_be_bytes());
        assert_eq!(read_bounded(data), Some(None), "returns, with nothing trimmed");
    }

    #[test]
    fn an_edit_whose_end_wraps_around_is_not_used() {
        // Version 1: a media time of i64::MAX, and a length that takes delay + frames past
        // 2^64 to a few frames.
        let delay = i64::MAX as u64;
        let frames = 0u64.wrapping_sub(delay) + 10;
        let dur = (u128::from(frames) * 1000).div_ceil(44_100) as u64;
        let mut elst = vec![1u8, 0, 0, 0];
        elst.extend(1u32.to_be_bytes());
        elst.extend(dur.to_be_bytes());
        elst.extend(delay.to_be_bytes());
        elst.extend([0, 1, 0, 0]);
        let p = file(Some(bx(b"edts", &bx(b"elst", &elst))), None);
        let data = std::fs::read(&p).unwrap();
        let _ = std::fs::remove_file(p);
        assert_eq!(read_bounded(data), Some(None), "nothing trimmed");
    }
}
