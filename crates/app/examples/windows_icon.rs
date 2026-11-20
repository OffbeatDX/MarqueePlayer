//! The app icon as a Windows `.ico` file, for the installer (Inno Setup's `SetupIconFile`):
//! the same artwork and sizes as the exe's icon resource (`build.rs`, `src/tray/icon_art.rs`).
//!
//!     cargo run --release -p marquee-player --example windows_icon -- <out.ico>
//!
//! (`cargo xtask release` runs it; the file is made at release time, not kept in the tree.)
//! Each size below 256 is a 32-bit DIB, as in the exe; 256 is a PNG, as Windows' own icons.

#[allow(dead_code)]
#[path = "../src/tray/icon_art.rs"]
mod icon_art;

const SIZES: [u32; 10] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

/// BITMAPINFOHEADER (double height), BGRA rows bottom-up, an all-zero AND mask.
fn dib(size: u32) -> Vec<u8> {
    let rgba = icon_art::rgba(size);
    let mask_row = size.div_ceil(32) * 4;
    let mut d = Vec::new();
    for v in [40u32, size, size * 2] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&32u16.to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes());
    d.extend_from_slice(&(size * size * 4 + mask_row * size).to_le_bytes());
    d.extend_from_slice(&[0u8; 16]);
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            d.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    d.resize(d.len() + (mask_row * size) as usize, 0);
    d
}

fn png(size: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(&mut out, &icon_art::rgba(size), size, size, image::ExtendedColorType::Rgba8, image::ImageFormat::Png).expect("encode the PNG");
    out.into_inner()
}

fn main() {
    let Some(out) = std::env::args_os().nth(1).map(std::path::PathBuf::from) else {
        eprintln!("usage: windows_icon <out.ico>");
        std::process::exit(2);
    };
    let images: Vec<(u32, Vec<u8>)> = SIZES.iter().map(|&s| (s, if s >= 256 { png(s) } else { dib(s) })).collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in &images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &images {
        ico.extend_from_slice(data);
    }
    std::fs::write(&out, ico).expect("write the .ico");
    println!("{}", out.display());
}
