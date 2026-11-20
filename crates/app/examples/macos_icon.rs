//! The app icon as PNGs in the sizes a macOS `.icns` holds, for the app bundle: the same
//! artwork as the Windows exe's and the Linux icons (`src/tray/icon_art.rs`), drawn on macOS'
//! icon grid (the tile 824 of 1024 units, centred, the rest transparent), so that it sits
//! in the Dock at the size of the system's own icons.
//!
//!     cargo run --release -p marquee-player --example macos_icon -- <out folder>
//!
//! writes `<out>/<n>.png` for each size; `cargo xtask release` runs it on macOS and packs
//! them into `MarqueePlayer.icns` (crates/xtask/src/release/macos.rs, which needs no
//! `iconutil`). The file is made at release time, not kept in the tree.

#[allow(dead_code)]
#[path = "../src/tray/icon_art.rs"]
mod icon_art;

/// The sizes the .icns entries take (16 to 512 points, at 1x and 2x).
const SIZES: [u32; 7] = [16, 32, 64, 128, 256, 512, 1024];

/// The tile on a transparent `size` square, at 824/1024 of it.
fn on_grid(size: u32) -> Vec<u8> {
    let inner = ((size as f32) * 824.0 / 1024.0).round() as u32;
    let off = (size - inner) / 2;
    let tile = icon_art::rgba(inner);
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..inner {
        let from = (y * inner * 4) as usize;
        let to = (((y + off) * size + off) * 4) as usize;
        out[to..to + (inner * 4) as usize].copy_from_slice(&tile[from..from + (inner * 4) as usize]);
    }
    out
}

fn main() {
    let Some(out) = std::env::args_os().nth(1).map(std::path::PathBuf::from) else {
        eprintln!("usage: macos_icon <output folder>");
        std::process::exit(2);
    };
    std::fs::create_dir_all(&out).expect("create the icon folder");
    for size in SIZES {
        let path = out.join(format!("{size}.png"));
        image::save_buffer(&path, &on_grid(size), size, size, image::ExtendedColorType::Rgba8).expect("write the icon");
        println!("{}", path.display());
    }
}
