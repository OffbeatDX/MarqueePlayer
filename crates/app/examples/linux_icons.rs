//! The app icon in the freedesktop hicolor sizes, for Linux packages (the AppImage, a
//! distribution package): the same artwork as the Windows exe's icon (`src/tray/icon_art.rs`).
//!
//!     cargo run --release -p marquee-player --example linux_icons -- packaging/linux/icons
//!
//! writes `<out>/hicolor/<n>x<n>/apps/app.marqueeplayer.MarqueePlayer.png` for each size.
//! (`packaging/linux/make-icons.sh` runs it.)

#[allow(dead_code)]
#[path = "../src/tray/icon_art.rs"]
mod icon_art;

/// The sizes desktops and panels look for (hicolor's usual set).
const SIZES: [u32; 9] = [16, 22, 24, 32, 48, 64, 128, 256, 512];
/// `crate::platform::APP_ID` (the example cannot use the app's modules).
const APP_ID: &str = "app.marqueeplayer.MarqueePlayer";

fn main() {
    let Some(out) = std::env::args_os().nth(1).map(std::path::PathBuf::from) else {
        eprintln!("usage: linux_icons <output folder>");
        std::process::exit(2);
    };
    for size in SIZES {
        let dir = out.join("hicolor").join(format!("{size}x{size}")).join("apps");
        std::fs::create_dir_all(&dir).expect("create the icon folder");
        let path = dir.join(format!("{APP_ID}.png"));
        image::save_buffer(&path, &icon_art::rgba(size), size, size, image::ExtendedColorType::Rgba8).expect("write the icon");
        println!("{}", path.display());
    }
}
