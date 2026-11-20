//! The tag editor's own look at a file's bytes, apart from lofty: every tag item as it is
//! stored (`Inventory`: an ID3v2 frame, a Vorbis comment, an MP4 `ilst` atom, a RIFF INFO
//! chunk...), and a print of everything that is not tags (`Layout`: the audio data and the
//! container's other structures, each hashed). Comparing the two before and after a write
//! shows anything lofty dropped or changed that its own reader would not show (a frame it
//! cannot parse, a comment with an odd key), and that the audio bytes are the same.
//!
//! Read-only: files are opened with `File::open`.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::hash::{self, Hasher};

/// One tag item as stored. `id` names it in its block's terms, upper-cased where the
/// format ignores case; `hash` is its stored bytes' hash, where they are compared (None:
/// compared by meaning instead, through lofty's reader: a text frame lofty re-encodes).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub block: &'static str,
    pub id: String,
    pub hash: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    pub entries: Vec<Entry>,
    /// The ID3v2 tag's version (3 or 4) and its padding, when there is one.
    pub id3v2: Option<(u8, u32)>,
    /// FLAC PADDING blocks' sizes.
    pub flac_padding: Vec<u32>,
}

/// Everything that is not tags, in order, hashed: (what, hash).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub parts: Vec<(String, u64)>,
}

#[derive(Clone, Debug, Default)]
pub struct RawView {
    pub inventory: Inventory,
    pub layout: Layout,
}

/// The container, as far as this module tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// MP3, MP2 or raw AAC: ID3v2 in front, APE and ID3v1 at the end, frames between.
    Frames,
    Flac,
    Ogg,
    Mp4,
    Riff,
    Aiff,
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn read_all(path: &Path) -> Result<Vec<u8>, String> {
    let mut f = std::fs::File::open(path).map_err(err)?;
    let mut v = Vec::new();
    f.read_to_end(&mut v).map_err(err)?;
    Ok(v)
}

/// Look at a file. Reads it whole: tags are near the start or the end, but the audio is
/// hashed too.
pub fn view(path: &Path, shape: Shape) -> Result<RawView, String> {
    let data = read_all(path)?;
    let mut v = RawView::default();
    match shape {
        Shape::Frames => frames_file(&data, &mut v)?,
        Shape::Flac => flac_file(&data, &mut v)?,
        Shape::Ogg => ogg_file(&data, &mut v)?,
        Shape::Mp4 => mp4_file(&data, &mut v)?,
        Shape::Riff => iff_file(&data, false, &mut v)?,
        Shape::Aiff => iff_file(&data, true, &mut v)?,
    }
    Ok(v)
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

fn synchsafe(b: &[u8], at: usize) -> Option<u32> {
    let x = b.get(at..at + 4)?;
    if x.iter().any(|v| v & 0x80 != 0) {
        return None;
    }
    Some((u32::from(x[0]) << 21) | (u32::from(x[1]) << 14) | (u32::from(x[2]) << 7) | u32::from(x[3]))
}

// ---- ID3v2 ------------------------------------------------------------------------------

/// The length of an ID3v2 tag at the start of `b` (header, body, footer), if there is one.
pub fn id3v2_len(b: &[u8]) -> Option<usize> {
    if b.len() < 10 || &b[..3] != b"ID3" {
        return None;
    }
    let size = synchsafe(b, 6)? as usize;
    let footer = if b[3] == 4 && b[5] & 0x10 != 0 { 10 } else { 0 };
    Some(10 + size + footer)
}

/// Undo ID3v2 unsynchronisation (FF 00 -> FF).
fn resync(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        out.push(b[i]);
        if b[i] == 0xFF && b.get(i + 1) == Some(&0) {
            i += 1;
        }
        i += 1;
    }
    out
}

/// Frames whose meaning lofty's reader gives (text, URLs, comments, lyrics, pictures,
/// owner-keyed data): compared through it. Everything else is compared byte for byte.
fn id3_compared_by_meaning(id: &str) -> bool {
    id.starts_with('T') || id.starts_with('W') || matches!(id, "COMM" | "USLT" | "APIC" | "PRIV" | "UFID" | "POPM" | "PCNT")
}

/// The frames of an ID3v2 tag (the whole tag, header first).
fn id3v2_frames(tag: &[u8], block: &'static str, inv: &mut Inventory) -> Result<(), String> {
    let version = tag[3];
    let flags = tag[5];
    if !(3..=4).contains(&version) {
        // ID3v2.2 (three-letter frames) is never written: see `edit::support`.
        inv.entries.push(Entry { block, id: format!("ID3v2.{version}"), hash: Some(hash::of(tag)) });
        inv.id3v2 = Some((version, 0));
        return Ok(());
    }
    let size = synchsafe(tag, 6).ok_or("bad ID3v2 size")? as usize;
    let body_end = (10 + size).min(tag.len());
    let mut body: Vec<u8> = tag[10..body_end].to_vec();
    if version == 3 && flags & 0x80 != 0 {
        body = resync(&body);
    }
    let mut at = 0usize;
    if flags & 0x40 != 0 {
        // Extended header: v2.3 its size excludes itself, v2.4's includes it.
        let n = if version == 3 { be32(&body, 0).map(|n| n as usize + 4) } else { synchsafe(&body, 0).map(|n| n as usize) };
        at = n.ok_or("bad ID3v2 extended header")?;
    }
    while at + 10 <= body.len() {
        let id_bytes = &body[at..at + 4];
        if id_bytes[0] == 0 {
            break; // padding
        }
        if !id_bytes.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            // Not a frame: junk where padding should be. Kept as it is (a hash of the rest).
            inv.entries.push(Entry { block, id: "(unparsed)".into(), hash: Some(hash::of(&body[at..])) });
            at = body.len();
            break;
        }
        let id = String::from_utf8_lossy(id_bytes).into_owned();
        let n = if version == 4 { synchsafe(&body, at + 4).or_else(|| be32(&body, at + 4)) } else { be32(&body, at + 4) }.ok_or("bad frame size")? as usize;
        let end = (at + 10 + n).min(body.len());
        let content = &body[at + 10..end];
        let hash = (!id3_compared_by_meaning(&id)).then(|| hash::of(content));
        inv.entries.push(Entry { block, id, hash });
        at = end;
    }
    let padding = body.len().saturating_sub(at) as u32;
    inv.id3v2 = Some((version, padding));
    Ok(())
}

// ---- MP3, MP2, raw AAC ------------------------------------------------------------------

/// Where the trailing tags start (APE, Lyrics3, ID3v1), and their inventory.
fn trailers(b: &[u8], start: usize, inv: &mut Inventory) -> usize {
    let mut end = b.len();
    if end >= start + 128 && &b[end - 128..end - 125] == b"TAG" {
        inv.entries.push(Entry { block: "ID3v1", id: "ID3v1".into(), hash: Some(hash::of(&b[end - 128..end])) });
        end -= 128;
    }
    // Lyrics3 v2: "LYRICS200" after a six-digit size.
    if end >= start + 15 && &b[end - 9..end] == b"LYRICS200" {
        let n = std::str::from_utf8(&b[end - 15..end - 9]).ok().and_then(|s| s.parse::<usize>().ok());
        if let Some(n) = n
            && end >= start + 15 + n
        {
            let at = end - 15 - n;
            inv.entries.push(Entry { block: "Lyrics3", id: "Lyrics3".into(), hash: Some(hash::of(&b[at..end])) });
            end = at;
        }
    }
    // APEv2 footer.
    if end >= start + 32 && &b[end - 32..end - 24] == b"APETAGEX" {
        let f = end - 32;
        let size = le32(b, f + 12).unwrap_or(0) as usize; // items and footer
        let count = le32(b, f + 16).unwrap_or(0) as usize;
        let flags = le32(b, f + 20).unwrap_or(0);
        let has_header = flags & (1 << 31) != 0;
        if size >= 32 && end >= start + size + if has_header { 32 } else { 0 } {
            let items_at = end - size;
            ape_items(&b[items_at..f], count, inv);
            end = items_at - if has_header { 32 } else { 0 };
        }
    }
    end
}

fn ape_items(b: &[u8], count: usize, inv: &mut Inventory) {
    let mut at = 0;
    for _ in 0..count {
        let (Some(len), Some(_flags)) = (le32(b, at), le32(b, at + 4)) else { break };
        let key_at = at + 8;
        let Some(nul) = b.get(key_at..).and_then(|r| r.iter().position(|c| *c == 0)) else { break };
        let key = String::from_utf8_lossy(&b[key_at..key_at + nul]).to_ascii_uppercase();
        let v_at = key_at + nul + 1;
        let v_end = (v_at + len as usize).min(b.len());
        inv.entries.push(Entry { block: "APE", id: key, hash: Some(hash::of(&b[at..v_end])) });
        at = v_end;
    }
}

fn frames_file(b: &[u8], v: &mut RawView) -> Result<(), String> {
    let start = match id3v2_len(b) {
        Some(n) if n <= b.len() => {
            id3v2_frames(&b[..n], "ID3v2", &mut v.inventory)?;
            n
        }
        _ => 0,
    };
    let end = trailers(b, start, &mut v.inventory);
    v.layout.parts.push(("audio".into(), hash::of(&b[start..end.max(start)])));
    Ok(())
}

// ---- FLAC -------------------------------------------------------------------------------

fn vorbis_comments(body: &[u8], block: &'static str, inv: &mut Inventory) -> Result<usize, String> {
    let vlen = le32(body, 0).ok_or("short comment block")? as usize;
    let mut at = 4 + vlen;
    let vendor = body.get(4..at).ok_or("short comment block")?;
    inv.entries.push(Entry { block, id: "(vendor)".into(), hash: Some(hash::of(vendor)) });
    let count = le32(body, at).ok_or("short comment block")? as usize;
    at += 4;
    for _ in 0..count {
        let n = le32(body, at).ok_or("short comment")? as usize;
        let c = body.get(at + 4..at + 4 + n).ok_or("short comment")?;
        // Names are case-insensitive (and lofty writes them upper-case): compared so.
        let eq = c.iter().position(|b| *b == b'=').unwrap_or(c.len());
        let key = String::from_utf8_lossy(&c[..eq]).to_ascii_uppercase();
        let mut h = crate::hash::Hasher::new();
        h.update(key.as_bytes());
        h.update(&c[eq..]);
        inv.entries.push(Entry { block, id: key, hash: Some(h.finish()) });
        at += 4 + n;
    }
    Ok(at)
}

fn flac_file(b: &[u8], v: &mut RawView) -> Result<(), String> {
    let mut at = match id3v2_len(b) {
        Some(n) if n <= b.len() => {
            id3v2_frames(&b[..n], "ID3v2", &mut v.inventory)?;
            n
        }
        _ => 0,
    };
    if b.get(at..at + 4) != Some(b"fLaC") {
        return Err("not a FLAC stream".into());
    }
    at += 4;
    loop {
        let h = b.get(at..at + 4).ok_or("truncated FLAC metadata")?;
        let last = h[0] & 0x80 != 0;
        let ty = h[0] & 0x7F;
        let n = (usize::from(h[1]) << 16) | (usize::from(h[2]) << 8) | usize::from(h[3]);
        let body = b.get(at + 4..at + 4 + n).ok_or("truncated FLAC block")?;
        match ty {
            1 => v.inventory.flac_padding.push(n as u32),
            4 => {
                vorbis_comments(body, "Vorbis", &mut v.inventory)?;
            }
            6 => v.inventory.entries.push(Entry { block: "Vorbis", id: "PICTURE".into(), hash: Some(hash::of(body)) }),
            _ => v.layout.parts.push((format!("FLAC block {ty}"), hash::of(body))),
        }
        at += 4 + n;
        if last {
            break;
        }
    }
    v.layout.parts.push(("audio".into(), hash::of(&b[at..])));
    Ok(())
}

// ---- Ogg ----------------------------------------------------------------------------------

/// One Ogg page's parts.
struct Page<'a> {
    flags: u8,
    granule: i64,
    serial: u32,
    segments: &'a [u8],
    body: &'a [u8],
}

fn pages(b: &[u8]) -> Result<Vec<Page<'_>>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() {
        if b.get(at..at + 4) != Some(b"OggS") {
            return Err(format!("no Ogg page at byte {at}"));
        }
        let h = b.get(at..at + 27).ok_or("truncated Ogg page")?;
        let n_seg = usize::from(h[26]);
        let segments = b.get(at + 27..at + 27 + n_seg).ok_or("truncated Ogg page")?;
        let len: usize = segments.iter().map(|s| usize::from(*s)).sum();
        let body_at = at + 27 + n_seg;
        let body = b.get(body_at..body_at + len).ok_or("truncated Ogg page")?;
        out.push(Page {
            flags: h[5],
            granule: i64::from_le_bytes(h[6..14].try_into().unwrap_or([0; 8])),
            serial: u32::from_le_bytes(h[14..18].try_into().unwrap_or([0; 4])),
            segments,
            body,
        });
        at = body_at + len;
    }
    Ok(out)
}

/// A logical stream's packets: (bytes, granule of the page it completed on when it is that
/// page's last completed packet, else -1).
/// A packet: its bytes, the granule (or -1), the page flags it ended on.
type Packet = (Vec<u8>, i64, u8);

fn packets(pages: &[Page<'_>]) -> BTreeMap<u32, Vec<Packet>> {
    let mut streams: BTreeMap<u32, (Vec<Packet>, Vec<u8>)> = BTreeMap::new();
    for p in pages {
        let (done, cur) = streams.entry(p.serial).or_default();
        let mut at = 0;
        let mut last_done = None;
        for &s in p.segments {
            let s = usize::from(s);
            cur.extend_from_slice(&p.body[at..at + s]);
            at += s;
            if s < 255 {
                done.push((std::mem::take(cur), -1, p.flags & 0x06));
                last_done = Some(done.len() - 1);
            }
        }
        if let Some(i) = last_done {
            done[i].1 = p.granule;
        }
    }
    streams.into_iter().map(|(k, (v, _))| (k, v)).collect()
}

fn ogg_file(b: &[u8], v: &mut RawView) -> Result<(), String> {
    let pages = pages(b)?;
    for (serial, pk) in packets(&pages) {
        let first = pk.first().map(|p| p.0.as_slice()).unwrap_or(&[]);
        let (comment_at, magic): (Option<usize>, usize) = if first.starts_with(b"OpusHead") {
            (Some(1), 8)
        } else if first.starts_with(b"\x01vorbis") {
            (Some(1), 7)
        } else if first.starts_with(b"Speex   ") {
            (Some(1), 0)
        } else {
            (None, 0)
        };
        let mut h = Hasher::new();
        h.number(u64::from(serial));
        for (i, (data, granule, flags)) in pk.iter().enumerate() {
            if Some(i) == comment_at {
                let body = data.get(magic..).unwrap_or(&[]);
                let end = vorbis_comments(body, "Vorbis", &mut v.inventory)?;
                // Opus: data after the comments (kept by RFC 7845 when its first bit is set).
                let rest = body.get(end..).unwrap_or(&[]);
                let rest = if first.starts_with(b"\x01vorbis") { rest.get(1..).unwrap_or(&[]) } else { rest };
                if !rest.is_empty() && rest.iter().any(|b| *b != 0) && (first.starts_with(b"OpusHead") && rest[0] & 1 == 1) {
                    v.inventory.entries.push(Entry { block: "Vorbis", id: "(binary after comments)".into(), hash: Some(hash::of(rest)) });
                }
                continue;
            }
            h.number(data.len() as u64);
            h.update(data);
            // Header packets complete on pages with granule 0 (or -1): only audio pages'
            // granules carry timing.
            let header = comment_at.is_some() && i <= 2 && *granule <= 0;
            h.number(if header { 0 } else { *granule as u64 });
            h.number(u64::from(*flags & 0x04)); // end of stream
        }
        v.layout.parts.push((format!("Ogg stream {serial:08x}"), h.finish()));
    }
    Ok(())
}

// ---- MP4 -------------------------------------------------------------------------------

/// The boxes in `b[from..to]`: (type, start of box, start of payload, end).
/// A box: its type, where it starts, where its payload starts, where it ends.
type BoxAt = ([u8; 4], usize, usize, usize);

fn boxes(b: &[u8], from: usize, to: usize) -> Result<Vec<BoxAt>, String> {
    let mut out = Vec::new();
    let mut at = from;
    while at + 8 <= to {
        let size32 = be32(b, at).ok_or("short box")? as u64;
        let ty: [u8; 4] = b[at + 4..at + 8].try_into().map_err(err)?;
        let (size, header) = match size32 {
            0 => ((to - at) as u64, 8usize),
            1 => {
                let x = b.get(at + 8..at + 16).ok_or("short box")?;
                (u64::from_be_bytes(x.try_into().map_err(err)?), 16)
            }
            n => (n, 8),
        };
        let end = at.checked_add(size as usize).filter(|e| *e <= to && size as usize >= header).ok_or_else(|| format!("bad MP4 box {}", String::from_utf8_lossy(&ty)))?;
        out.push((ty, at, at + header, end));
        at = end;
    }
    Ok(out)
}

const MP4_CONTAINERS: &[&[u8; 4]] = &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"edts", b"dinf", b"mvex"];

/// Hash a box tree, with chunk offsets taken relative to the media data (`base`), so a
/// `moov` that grew in front of `mdat` hashes the same.
fn hash_box(b: &[u8], ty: &[u8; 4], start: usize, payload: usize, end: usize, base: u64, h: &mut Hasher) -> Result<(), String> {
    h.update(ty);
    if MP4_CONTAINERS.contains(&ty) {
        for (t, s, p, e) in boxes(b, payload, end)? {
            if &t == b"udta" || &t == b"free" || &t == b"skip" || &t == b"meta" {
                continue; // tags (and the space around them)
            }
            hash_box(b, &t, s, p, e, base, h)?;
        }
        return Ok(());
    }
    if ty == b"stco" || ty == b"co64" {
        let wide = ty == b"co64";
        let n = be32(b, payload + 4).unwrap_or(0) as usize;
        h.update(&b[payload..payload + 8]);
        for i in 0..n {
            let off = if wide {
                b.get(payload + 8 + i * 8..payload + 16 + i * 8).map(|x| u64::from_be_bytes(x.try_into().unwrap_or([0; 8])))
            } else {
                be32(b, payload + 8 + i * 4).map(u64::from)
            }
            .ok_or("short chunk offsets")?;
            h.number(off.wrapping_sub(base));
        }
        return Ok(());
    }
    h.update(&b[start..end]);
    Ok(())
}

fn ilst_items(b: &[u8], payload: usize, end: usize, inv: &mut Inventory) -> Result<(), String> {
    for (ty, _s, p, e) in boxes(b, payload, end)? {
        let mut id = String::from_utf8_lossy(&ty).into_owned();
        if ty[0] == 0xA9 {
            id = format!("\u{a9}{}", String::from_utf8_lossy(&ty[1..]));
        }
        let mut h = Hasher::new();
        if &ty == b"----" {
            let (mut mean, mut name) = (String::new(), String::new());
            for (t, _, cp, ce) in boxes(b, p, e)? {
                let text = String::from_utf8_lossy(b.get(cp + 4..ce).unwrap_or(&[])).into_owned();
                match &t {
                    b"mean" => mean = text,
                    b"name" => name = text,
                    _ => h.update(&b[cp..ce]),
                }
            }
            id = format!("----:{mean}:{name}");
        } else {
            for (t, _, cp, ce) in boxes(b, p, e)? {
                h.update(&t);
                h.update(&b[cp..ce]);
            }
        }
        inv.entries.push(Entry { block: "MP4", id, hash: Some(h.finish()) });
    }
    Ok(())
}

fn find_ilst(b: &[u8], payload: usize, end: usize, inv: &mut Inventory) -> Result<(), String> {
    for (ty, _s, p, e) in boxes(b, payload, end)? {
        match &ty {
            b"udta" => find_ilst(b, p, e, inv)?,
            // `meta` is a full box: four bytes of version and flags before its children.
            b"meta" => find_ilst(b, p + 4, e, inv)?,
            b"ilst" => ilst_items(b, p, e, inv)?,
            _ => {}
        }
    }
    Ok(())
}

fn mp4_file(b: &[u8], v: &mut RawView) -> Result<(), String> {
    let top = boxes(b, 0, b.len())?;
    let base = top.iter().find(|(t, ..)| t == b"mdat").map(|(_, _, p, _)| *p as u64).ok_or("no media data")?;
    for (ty, s, p, e) in &top {
        match ty {
            b"moov" => {
                let mut h = Hasher::new();
                hash_box(b, ty, *s, *p, *e, base, &mut h)?;
                v.layout.parts.push(("moov".into(), h.finish()));
                find_ilst(b, *p, *e, &mut v.inventory)?;
            }
            b"mdat" => v.layout.parts.push(("mdat".into(), hash::of(&b[*p..*e]))),
            b"free" | b"skip" => {}
            _ => v.layout.parts.push((String::from_utf8_lossy(ty).into_owned(), hash::of(&b[*s..*e]))),
        }
    }
    Ok(())
}

// ---- WAV and AIFF -------------------------------------------------------------------------

fn iff_file(b: &[u8], big: bool, v: &mut RawView) -> Result<(), String> {
    let n32 = |at: usize| if big { be32(b, at) } else { le32(b, at) };
    let form: &[u8] = if big { b"FORM" } else { b"RIFF" };
    if b.get(..4) != Some(form) {
        return Err("not a RIFF or AIFF file".into());
    }
    let end = (8 + n32(4).ok_or("short header")? as usize).min(b.len());
    v.layout.parts.push(("form type".into(), hash::of(b.get(8..12).unwrap_or(&[]))));
    let mut at = 12;
    while at + 8 <= end {
        let id: [u8; 4] = b[at..at + 4].try_into().map_err(err)?;
        let n = n32(at + 4).ok_or("short chunk")? as usize;
        let body_end = (at + 8 + n).min(end);
        let body = &b[at + 8..body_end];
        let name = String::from_utf8_lossy(&id).into_owned();
        match &id {
            b"ID3 " | b"id3 " if body.len() >= 10 => id3v2_frames(body, "ID3v2", &mut v.inventory)?,
            b"LIST" if !big && body.get(..4) == Some(b"INFO") => {
                let mut i = 4;
                while i + 8 <= body.len() {
                    let sub = String::from_utf8_lossy(&body[i..i + 4]).into_owned();
                    let sn = le32(body, i + 4).unwrap_or(0) as usize;
                    let se = (i + 8 + sn).min(body.len());
                    v.inventory.entries.push(Entry { block: "RIFF INFO", id: sub, hash: Some(hash::of(&body[i + 8..se])) });
                    i = se + (sn & 1);
                }
            }
            b"NAME" | b"AUTH" | b"(c) " | b"ANNO" | b"COMT" if big => {
                v.inventory.entries.push(Entry { block: "AIFF text", id: name, hash: Some(hash::of(body)) });
            }
            _ => v.layout.parts.push((name, hash::of(body))),
        }
        at = body_end + (n & 1);
    }
    Ok(())
}

/// Read `len` bytes at `at` of a file (for small checks without reading it whole).
pub fn read_at(path: &Path, at: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(at))?;
    let mut v = vec![0; len];
    let n = f.read(&mut v)?;
    v.truncate(n);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id3v2_frames_and_padding() {
        // v2.4: TIT2 "Hi", a PRIV frame, then 6 bytes of padding.
        let mut body = Vec::new();
        body.extend_from_slice(b"TIT2\0\0\0\x03\0\0\x03Hi");
        body.extend_from_slice(b"ABCD\0\0\0\x02\0\0xy");
        body.extend_from_slice(&[0; 6]);
        let mut tag = b"ID3\x04\0\0".to_vec();
        let n = body.len() as u32;
        tag.extend_from_slice(&[(n >> 21) as u8 & 0x7F, (n >> 14) as u8 & 0x7F, (n >> 7) as u8 & 0x7F, n as u8 & 0x7F]);
        tag.extend_from_slice(&body);
        assert_eq!(id3v2_len(&tag), Some(tag.len()));
        let mut inv = Inventory::default();
        id3v2_frames(&tag, "ID3v2", &mut inv).unwrap();
        assert_eq!(inv.entries.len(), 2);
        assert_eq!(inv.entries[0], Entry { block: "ID3v2", id: "TIT2".into(), hash: None });
        assert_eq!(inv.entries[1].hash, Some(hash::of(b"xy")), "an unknown frame by its bytes");
        assert_eq!(inv.id3v2, Some((4, 6)));
        assert_eq!(resync(&[0xFF, 0, 0xE0, 0xFF, 1]), [0xFF, 0xE0, 0xFF, 1]);
    }

    #[test]
    fn trailing_tags_are_found() {
        let mut b = vec![0xFFu8; 100];
        b.extend_from_slice(b"TAG");
        b.extend_from_slice(&[b' '; 125]);
        let mut inv = Inventory::default();
        assert_eq!(trailers(&b, 0, &mut inv), 100);
        assert_eq!(inv.entries[0].id, "ID3v1");
    }

    #[test]
    fn ogg_pages_give_packets_with_granules() {
        // Two pages of one stream: packet A (300 bytes, over two segments), then B.
        let page = |flags: u8, granule: i64, segs: &[u8], body: &[u8]| {
            let mut p = b"OggS\0".to_vec();
            p.push(flags);
            p.extend_from_slice(&granule.to_le_bytes());
            p.extend_from_slice(&7u32.to_le_bytes());
            p.extend_from_slice(&[0; 8]);
            p.push(segs.len() as u8);
            p.extend_from_slice(segs);
            p.extend_from_slice(body);
            p
        };
        let mut b = page(2, 0, &[255, 45], &[1; 300]);
        b.extend(page(4, 960, &[10], &[2; 10]));
        let ps = pages(&b).unwrap();
        let pk = packets(&ps);
        let s = &pk[&7];
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].0.len(), s[0].1), (300, 0));
        assert_eq!((s[1].0.len(), s[1].1), (10, 960));
    }
}
