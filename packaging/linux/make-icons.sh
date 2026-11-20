#!/bin/sh
# Renders the app icon in the hicolor sizes, from the artwork the Windows exe's icon is made
# of (crates/app/src/tray/icon_art.rs), into packaging/linux/icons/hicolor/<n>x<n>/apps/.
# Run again when the artwork changes. Needs a Rust toolchain (it builds a small example).
#   packaging/linux/make-icons.sh
set -e
cd "$(dirname "$0")/../.."
cargo run --release -p marquee-player --example linux_icons -- packaging/linux/icons
