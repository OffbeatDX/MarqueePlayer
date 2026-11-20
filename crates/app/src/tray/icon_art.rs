//! The app icon's artwork, drawn in code: a dark navy tile with a
//! soft glow, and the app's glyph on it, a sound wave in amber running to pink.
//! Dependency-free so `build.rs` can include it too: the
//! same pixels become the exe's icon resource (Explorer, taskbar, title bar), and the
//! tray gets the glyph alone, which stays readable at 16 px.
//!
//! Everything is laid out on a 256-unit square and sampled 4x4 per output pixel.

const NAVY_IN: [f32; 3] = [26.0, 36.0, 64.0]; // #1a2440
const NAVY_OUT: [f32; 3] = [7.0, 10.0, 18.0]; // #070a12
const WARM_A: [f32; 3] = [255.0, 198.0, 92.0]; // #ffc65c
const WARM_B: [f32; 3] = [255.0, 77.0, 184.0]; // #ff4db8
// The same gradient, deeper, for a light taskbar (the bright amber is lost on near white).
const WARM_A_LIGHT: [f32; 3] = [214.0, 128.0, 0.0]; // #d68000
const WARM_B_LIGHT: [f32; 3] = [214.0, 30.0, 140.0]; // #d61e8c

/// The wave: (left edge, height) of each bar; all 24 wide, centred on y = 128.
const BARS: [(f32, f32); 5] = [(40.0, 56.0), (78.0, 124.0), (116.0, 172.0), (154.0, 104.0), (192.0, 64.0)];
const BAR_W: f32 = 24.0;
/// The bars' heights at rest (the static glyph), in the 256-unit square.
#[allow(dead_code)]
pub const REST: [f32; 5] = [BARS[0].1, BARS[1].1, BARS[2].1, BARS[3].1, BARS[4].1];

/// The icon on its tile. Straight (not premultiplied) RGBA, row-major, top row first.
pub fn rgba(size: u32) -> Vec<u8> {
    // Small sizes get a larger glyph: at 16 px the full-size one is a smudge.
    draw(size, true, if size >= 32 { 1.0 } else { 1.22 })
}

/// The glyph alone on a transparent ground, filling the square: the tray icon.
#[allow(dead_code)] // build.rs includes this file and embeds only the tile.
pub fn glyph_rgba(size: u32) -> Vec<u8> {
    glyph_with(size, &REST)
}

/// The glyph with its bars at these heights (the 256-unit square, before the glyph's
/// scale; `REST` is the static glyph): the tray's animation. Same shapes and gradient.
#[allow(dead_code)]
pub fn glyph_with(size: u32, heights: &[f32; 5]) -> Vec<u8> {
    draw_with(size, false, 1.42, heights, false)
}

/// `glyph_with`, in the deeper gradient when `light` (a light taskbar).
#[allow(dead_code)]
pub fn glyph_on(size: u32, heights: &[f32; 5], light: bool) -> Vec<u8> {
    draw_with(size, false, 1.42, heights, light)
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Signed distance to a rounded rectangle (negative inside).
fn round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let (qx, qy) = ((px - cx).abs() - (hw - r), (py - cy).abs() - (hh - r));
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// The colour at a point in the 256-unit square, with its coverage (0 or 1).
fn sample(u: f32, v: f32, tile: bool, scale: f32, heights: &[f32; 5], light: bool) -> Option<[f32; 3]> {
    // The glyph, scaled about the centre.
    let (gx, gy) = ((u - 128.0) / scale + 128.0, (v - 128.0) / scale + 128.0);
    let in_glyph = BARS.iter().zip(heights).any(|(&(x, _), &h)| {
        // Never shorter than its width: at the least a round dot.
        let h = h.max(BAR_W);
        round_rect(gx, gy, x + BAR_W / 2.0, 128.0, BAR_W / 2.0, h / 2.0, BAR_W / 2.0) <= 0.0
    });
    if in_glyph {
        // One gradient across the whole wave, top left to bottom right.
        let (x0, y0, x1, y1) = (57.6, 42.0, 198.4, 214.0);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let t = (((gx - x0) * dx + (gy - y0) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        return Some(if light { lerp(WARM_A_LIGHT, WARM_B_LIGHT, t) } else { lerp(WARM_A, WARM_B, t) });
    }
    if !tile {
        return None;
    }
    let edge = round_rect(u, v, 128.0, 128.0, 124.0, 124.0, 58.0);
    if edge > 0.0 {
        return None;
    }
    let from = |cx: f32, cy: f32| ((u - cx).powi(2) + (v - cy).powi(2)).sqrt();
    let mut c = lerp(NAVY_IN, NAVY_OUT, (from(128.0, 107.5) / 186.0).min(1.0));
    // The glyph's glow.
    c = lerp(c, WARM_A, (1.0 - from(128.0, 128.0) / 112.0).max(0.0) * 0.34);
    // A faint lighter rim.
    if edge > -1.5 {
        c = lerp(c, [255.0; 3], 0.08);
    }
    Some(c)
}

fn draw(size: u32, tile: bool, scale: f32) -> Vec<u8> {
    draw_with(size, tile, scale, &REST, false)
}

fn draw_with(size: u32, tile: bool, scale: f32, heights: &[f32; 5], light: bool) -> Vec<u8> {
    const N: u32 = 4;
    let unit = 256.0 / size as f32;
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut sum, mut hits) = ([0.0f32; 3], 0u32);
            for j in 0..N {
                for i in 0..N {
                    let u = (x as f32 + (i as f32 + 0.5) / N as f32) * unit;
                    let v = (y as f32 + (j as f32 + 0.5) / N as f32) * unit;
                    if let Some(c) = sample(u, v, tile, scale, heights, light) {
                        sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
                        hits += 1;
                    }
                }
            }
            if hits == 0 {
                px.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let n = hits as f32;
                let a = (n / (N * N) as f32 * 255.0).round() as u8;
                px.extend_from_slice(&[(sum[0] / n).round() as u8, (sum[1] / n).round() as u8, (sum[2] / n).round() as u8, a]);
            }
        }
    }
    px
}
