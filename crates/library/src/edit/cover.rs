//! A picked image made ready to embed as a front cover: kept as it is when it is a JPEG or
//! PNG of sensible size, else decoded and saved again as a JPEG (quality 90) at most
//! `MAX_SIDE` pixels on its longest side. Transparency is laid on white for a JPEG.

use std::path::Path;

use super::PictureData;

/// A cover larger than this on its longest side is made smaller.
pub const MAX_SIDE: u32 = 2000;
/// A JPEG or PNG larger than this is saved again (smaller), whatever its size in pixels.
pub const MAX_BYTES: usize = 3 << 20;
/// Images larger than this are not read at all.
pub const MAX_SOURCE_BYTES: u64 = 100 << 20;
pub const JPEG_QUALITY: u8 = 90;

#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    pub picture: PictureData,
    pub width: u32,
    pub height: u32,
    /// What was picked: its format, pixel size and bytes.
    pub source: (String, u32, u32, u64),
    pub reencoded: bool,
}

impl Prepared {
    /// "JPEG · 1,200 × 1,200 px · 312 KB, from a 6,000 × 6,000 PNG of 48 MB".
    pub fn describe(&self) -> (String, Option<String>) {
        let fmt = if self.picture.mime == "image/png" { "PNG" } else { "JPEG" };
        let main = format!("{fmt} \u{b7} {} \u{d7} {} px \u{b7} {}", crate::audio::thousands(u64::from(self.width)), crate::audio::thousands(u64::from(self.height)), size_text(self.picture.data.len() as u64));
        let from = self.reencoded.then(|| format!("Made from a {} \u{d7} {} {} of {}", crate::audio::thousands(u64::from(self.source.1)), crate::audio::thousands(u64::from(self.source.2)), self.source.0, size_text(self.source.3)));
        (main, from)
    }
}

pub fn size_text(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / f64::from(1u32 << 20))
    } else {
        format!("{} KB", n.div_ceil(1024))
    }
}

pub fn prepare(path: &Path) -> Result<Prepared, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_SOURCE_BYTES {
        return Err(format!("The image is {}: pick one under {}", size_text(len), size_text(MAX_SOURCE_BYTES)));
    }
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    prepare_bytes(data)
}

pub fn prepare_bytes(data: Vec<u8>) -> Result<Prepared, String> {
    let format = image::guess_format(&data).map_err(|_| "Not an image this app can read (JPEG, PNG or WebP)".to_owned())?;
    let name = match format {
        image::ImageFormat::Jpeg => "JPEG",
        image::ImageFormat::Png => "PNG",
        image::ImageFormat::WebP => "WebP",
        _ => return Err("Not an image this app can read (JPEG, PNG or WebP)".into()),
    };
    let (w, h) = crate::meta::image_size(&data).ok_or("The image cannot be read")?;
    if w == 0 || h == 0 {
        return Err("The image is empty".into());
    }
    let source = (name.to_owned(), w, h, data.len() as u64);
    let keep = matches!(format, image::ImageFormat::Jpeg | image::ImageFormat::Png) && w.max(h) <= MAX_SIDE && data.len() <= MAX_BYTES;
    if keep {
        let mime = if format == image::ImageFormat::Png { "image/png" } else { "image/jpeg" };
        return Ok(Prepared { picture: PictureData { type_code: 3, mime: mime.into(), description: String::new(), data }, width: w, height: h, source, reencoded: false });
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&data), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1 << 30);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| format!("The image cannot be read ({e})"))?;
    let img = if w.max(h) > MAX_SIDE { img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Lanczos3) } else { img };
    let rgba = img.to_rgba8();
    let (nw, nh) = rgba.dimensions();
    let mut rgb = image::RgbImage::new(nw, nh);
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = u32::from(p[3]);
        let mix = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
        rgb.put_pixel(x, y, image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])]));
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode_image(&rgb).map_err(|e| e.to_string())?;
    Ok(Prepared { picture: PictureData { type_code: 3, mime: "image/jpeg".into(), description: String::new(), data: out }, width: nw, height: nh, source, reencoded: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, if x < 4 { 0 } else { 255 }]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn small_covers_are_kept_and_enormous_ones_made_smaller() {
        let small = png(600, 400);
        let p = prepare_bytes(small.clone()).unwrap();
        assert!(!p.reencoded);
        assert_eq!((p.width, p.height, p.picture.mime.as_str()), (600, 400, "image/png"));
        assert_eq!(p.picture.data, small);
        let big = png(4000, 3000);
        let p = prepare_bytes(big).unwrap();
        assert!(p.reencoded);
        assert_eq!((p.width, p.height, p.picture.mime.as_str()), (2000, 1500, "image/jpeg"));
        assert_eq!(crate::meta::image_size(&p.picture.data), Some((2000, 1500)));
        assert!(p.describe().1.unwrap().contains("4,000 \u{d7} 3,000 PNG"));
        assert!(prepare_bytes(b"not an image".to_vec()).is_err());
    }
}
