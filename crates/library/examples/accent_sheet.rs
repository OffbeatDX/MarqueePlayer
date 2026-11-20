//! Contact sheet of accents for cached covers: cover, raw extracted colour, usable accent
//! (with a dark-text bar on it). `cargo run -p library --example accent_sheet -- <art dir> <out.png> [count]`
use image::{Rgb, RgbImage, imageops};
use library::accent;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("art dir"));
    let out = args.next().expect("out png");
    let count: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(48);
    let mut thumbs: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with("_t.jpg")).collect();
    thumbs.sort();
    let step = (thumbs.len() / count).max(1);
    let picked: Vec<_> = thumbs.iter().step_by(step).take(count).collect();
    let (cell_w, cell_h, cols) = (96 * 3, 96, 4);
    let rows = picked.len().div_ceil(cols);
    let mut sheet = RgbImage::from_pixel((cell_w * cols) as u32, (cell_h * rows) as u32, Rgb([12, 11, 16]));
    let (mut none, mut total) = (0, 0);
    for (i, p) in picked.iter().enumerate() {
        let img = image::open(p).unwrap();
        let small = img.resize_exact(64, 64, imageops::FilterType::Triangle).to_rgb8();
        let raw = accent::extract(small.pixels().map(|p| p.0));
        total += 1;
        let (x, y) = (((i % cols) * cell_w) as i64, ((i / cols) * cell_h) as i64);
        let cover = img.resize_exact(92, 92, imageops::FilterType::Triangle).to_rgb8();
        imageops::overlay(&mut sheet, &cover, x + 2, y + 2);
        let (rawc, usable, text) = match raw {
            Some(c) => { let (u, t) = accent::usable(c); (c, u, t) }
            None => { none += 1; ([60, 60, 60], [0xb4, 0xa6, 0xe8], [0x1e, 0x18, 0x2c]) }
        };
        let swatch = |sheet: &mut RgbImage, ox: i64, c: [u8; 3]| {
            for yy in 4..92 { for xx in 4..92 { sheet.put_pixel((x + ox + xx) as u32, (y + yy) as u32, Rgb(c)); } }
        };
        swatch(&mut sheet, 96, rawc);
        swatch(&mut sheet, 192, usable);
        for yy in 40..52 { for xx in 20..76 { sheet.put_pixel((x + 192 + xx) as u32, (y + yy) as u32, Rgb(text)); } }
    }
    sheet.save(&out).unwrap();
    println!("{total} covers, {none} without an accent (fallback shown)");
}
