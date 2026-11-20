//! The Slint UI (`crates/app/ui/*.slint`), compiled to Rust. The app uses it as if it were
//! its own module (`use marquee_ui::*` at its crate root). It is a separate crate only so
//! that it can be optimized for size while the app stays optimized for speed (see the
//! profile in the workspace Cargo.toml).
slint::include_modules!();
