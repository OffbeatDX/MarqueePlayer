//! The accent colour of a cover: a colour that belongs to the art, made safe to use as
//! the UI accent on a dark background.
//!
//! Extraction (`extract`): a plain average of a cover is a muddy grey-brown, so this
//! picks the most prominent *vivid* hue instead. Every pixel is converted to OKLCH;
//! near-black, near-white and near-grey pixels are ignored; the rest vote into 24 hue
//! sectors, each vote weighted by chroma (vivid pixels count more) and by how close its
//! lightness is to the middle. The winning sector, smoothed with its neighbours, gives
//! the hue; the colour is the vote-weighted mean of that sector's pixels in Oklab.
//! Art with too few vivid pixels (greyscale, almost all black or white) has no accent.
//!
//! Clamping (`usable`): lightness and chroma are clamped in OKLCH so the accent reads as
//! a colour, has contrast against the dark background for text, icons and the seek bar,
//! and carries dark text on accent-filled buttons; out-of-gamut results are brought in by
//! lowering chroma. The tests check contrast on awkward covers.

/// Linear sRGB channel from 8-bit sRGB.
fn to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Oklab (L, a, b) from 8-bit sRGB.
pub fn oklab([r, g, b]: [u8; 3]) -> [f32; 3] {
    let (r, g, b) = (to_linear(r), to_linear(g), to_linear(b));
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Linear sRGB from Oklab, unclamped (used to test the gamut).
fn oklab_to_linear([l, a, b]: [f32; 3]) -> [f32; 3] {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3,
        -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3,
    ]
}

pub fn oklab_to_srgb(lab: [f32; 3]) -> [u8; 3] {
    let [r, g, b] = oklab_to_linear(lab);
    let q = |c: f32| (from_linear(c) * 255.0).round() as u8;
    [q(r), q(g), q(b)]
}

/// OKLCH (L, C, h in radians) from Oklab, and back.
pub fn to_lch([l, a, b]: [f32; 3]) -> [f32; 3] {
    [l, a.hypot(b), b.atan2(a)]
}

pub fn from_lch([l, c, h]: [f32; 3]) -> [f32; 3] {
    [l, c * h.cos(), c * h.sin()]
}

fn in_gamut(lab: [f32; 3]) -> bool {
    oklab_to_linear(lab).iter().all(|c| (-0.0005..=1.0005).contains(c))
}

const SECTORS: usize = 24;
/// Pixels darker or lighter than this (Oklab L) do not vote: shadows and paper.
const MIN_L: f32 = 0.22;
const MAX_L: f32 = 0.975;
/// Pixels less colourful than this (Oklab chroma) do not vote: greys.
const MIN_C: f32 = 0.045;
/// Below this share of the image's possible vote, the art counts as colourless.
const MIN_SHARE: f32 = 0.001;

/// The most prominent vivid colour of an image given as RGB pixels, unclamped (as it is
/// in the art). `None` for greyscale or nearly colourless art.
pub fn extract(pixels: impl Iterator<Item = [u8; 3]>) -> Option<[u8; 3]> {
    let mut weight = [0f32; SECTORS];
    let mut sum = [[0f32; 3]; SECTORS];
    let mut n = 0usize;
    for px in pixels {
        n += 1;
        let lab = oklab(px);
        let [l, c, h] = to_lch(lab);
        if !(MIN_L..=MAX_L).contains(&l) || c < MIN_C {
            continue;
        }
        // Chroma counts most; mid lightness a little more than the extremes.
        let w = c * c * (1.0 - (l - 0.6).abs());
        let sector = (((h + std::f32::consts::PI) / std::f32::consts::TAU) * SECTORS as f32) as usize % SECTORS;
        weight[sector] += w;
        for k in 0..3 {
            sum[sector][k] += lab[k] * w;
        }
    }
    if n == 0 {
        return None;
    }
    // Neighbouring sectors smooth the vote, so a hue on a sector boundary is not split.
    let smoothed = |i: usize| weight[(i + SECTORS - 1) % SECTORS] * 0.5 + weight[i] + weight[(i + 1) % SECTORS] * 0.5;
    let best = (0..SECTORS).max_by(|&a, &b| smoothed(a).total_cmp(&smoothed(b)))?;
    // The strongest possible vote of one pixel is about 0.1 (chroma ~0.32 squared).
    if smoothed(best) < MIN_SHARE * 0.1 * n as f32 || weight[best] <= 0.0 {
        return None;
    }
    let mean = [sum[best][0] / weight[best], sum[best][1] / weight[best], sum[best][2] / weight[best]];
    Some(oklab_to_srgb(mean))
}

/// Up to `n` colours of an image, the most prominent first, each a different hue (at least
/// three sectors, 45°, apart): the same vote as `extract`, read further down. For a
/// gradient of the cover's own colours (the visualizer). Each is made vivid and light
/// enough to read on the dark theme (lightness 0.55-0.88, chroma at least 0.09), so a
/// muted cover still gives a gradient. Empty for colourless art.
pub fn palette(pixels: impl Iterator<Item = [u8; 3]>, n: usize) -> Vec<[u8; 3]> {
    let mut weight = [0f32; SECTORS];
    let mut sum = [[0f32; 3]; SECTORS];
    let mut count = 0usize;
    for px in pixels {
        count += 1;
        let lab = oklab(px);
        let [l, c, h] = to_lch(lab);
        if !(MIN_L..=MAX_L).contains(&l) || c < MIN_C {
            continue;
        }
        let w = c * c * (1.0 - (l - 0.6).abs());
        let sector = (((h + std::f32::consts::PI) / std::f32::consts::TAU) * SECTORS as f32) as usize % SECTORS;
        weight[sector] += w;
        for k in 0..3 {
            sum[sector][k] += lab[k] * w;
        }
    }
    let smoothed = |i: usize| weight[(i + SECTORS - 1) % SECTORS] * 0.5 + weight[i] + weight[(i + 1) % SECTORS] * 0.5;
    let mut order: Vec<usize> = (0..SECTORS).collect();
    order.sort_by(|&a, &b| smoothed(b).total_cmp(&smoothed(a)));
    let floor = MIN_SHARE * 0.1 * count as f32;
    let mut picked: Vec<usize> = Vec::new();
    for s in order {
        if picked.len() >= n || smoothed(s) < floor.max(smoothed(picked.first().copied().unwrap_or(s)) * 0.06) || weight[s] <= 0.0 {
            continue;
        }
        let near = picked.iter().any(|&p| {
            let d = (p as i32 - s as i32).rem_euclid(SECTORS as i32);
            d.min(SECTORS as i32 - d) < 3
        });
        if !near {
            picked.push(s);
        }
    }
    picked
        .into_iter()
        .map(|s| {
            let lab = [sum[s][0] / weight[s], sum[s][1] / weight[s], sum[s][2] / weight[s]];
            let [l, c, h] = to_lch(lab);
            let (l, mut c) = (l.clamp(0.55, 0.88), c.max(0.09));
            while c > 0.0 && !in_gamut(from_lch([l, c, h])) {
                c -= 0.005;
            }
            oklab_to_srgb(from_lch([l, c.max(0.0), h]))
        })
        .collect()
}

/// Clamp bounds for an accent on the dark theme.
pub const L_RANGE: (f32, f32) = (0.74, 0.86);
pub const C_RANGE: (f32, f32) = (0.07, 0.16);

/// A colour made usable as the accent: lightness and chroma clamped (OKLCH), then
/// brought into the sRGB gamut by lowering chroma. Also returns the colour for text on
/// accent-filled buttons: the same hue, very dark.
pub fn usable(rgb: [u8; 3]) -> ([u8; 3], [u8; 3]) {
    let [l, c, h] = to_lch(oklab(rgb));
    let l = l.clamp(L_RANGE.0, L_RANGE.1);
    let mut c = c.clamp(C_RANGE.0, C_RANGE.1);
    while c > 0.0 && !in_gamut(from_lch([l, c, h])) {
        c -= 0.005;
    }
    let accent = oklab_to_srgb(from_lch([l, c.max(0.0), h]));
    let text = oklab_to_srgb(from_lch([0.2, 0.035, h]));
    (accent, text)
}

/// WCAG relative luminance and contrast ratio, for tests and checks.
pub fn luminance([r, g, b]: [u8; 3]) -> f32 {
    0.2126 * to_linear(r) + 0.7152 * to_linear(g) + 0.0722 * to_linear(b)
}

pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// One step of a perceptual transition: OKLCH, hue along the shorter way round, so a
/// change between complementary colours does not pass through grey.
pub fn mix(from: [u8; 3], to: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let [l0, c0, h0] = to_lch(oklab(from));
    let [l1, c1, h1] = to_lch(oklab(to));
    // A near-grey end has no meaningful hue: take the other end's.
    let (h0, h1) = match (c0 < 0.02, c1 < 0.02) {
        (true, false) => (h1, h1),
        (false, true) => (h0, h0),
        _ => (h0, h1),
    };
    let mut dh = h1 - h0;
    if dh > std::f32::consts::PI {
        dh -= std::f32::consts::TAU;
    } else if dh < -std::f32::consts::PI {
        dh += std::f32::consts::TAU;
    }
    let lch = [l0 + (l1 - l0) * t, c0 + (c1 - c0) * t, h0 + dh * t];
    let mut lab = from_lch(lch);
    // Keep intermediate steps in gamut the same way as the ends.
    let mut c = lch[1];
    while c > 0.0 && !in_gamut(lab) {
        c -= 0.005;
        lab = from_lch([lch[0], c, lch[2]]);
    }
    oklab_to_srgb(lab)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BG: [u8; 3] = [0x0c, 0x0b, 0x10];

    fn image(f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<[u8; 3]> {
        (0..64 * 64).map(|i| f(i % 64, i / 64)).collect()
    }

    fn assert_usable(rgb: [u8; 3]) {
        let (accent, text) = usable(rgb);
        assert!(contrast(accent, BG) >= 7.0, "{accent:?} on bg: {}", contrast(accent, BG));
        assert!(contrast(accent, text) >= 7.0, "{text:?} on {accent:?}: {}", contrast(accent, text));
        let [_, c, _] = to_lch(oklab(accent));
        assert!(c >= 0.05, "accent {accent:?} is grey (chroma {c})");
    }

    #[test]
    fn oklab_round_trips() {
        for rgb in [[0, 0, 0], [255, 255, 255], [255, 179, 71], [12, 200, 90], [30, 60, 250]] {
            assert_eq!(oklab_to_srgb(oklab(rgb)), rgb);
        }
    }

    #[test]
    fn vivid_patch_beats_a_larger_dull_background() {
        // Mostly grey-brown, with a red quarter: the accent is red, not the average.
        let img = image(|x, y| if x < 32 && y < 32 { [220, 30, 40] } else { [110, 100, 90] });
        let c = extract(img.into_iter()).unwrap();
        let [_, _, h] = to_lch(oklab(c));
        let [_, _, red] = to_lch(oklab([220, 30, 40]));
        assert!((h - red).abs() < 0.2, "{c:?}");
    }

    #[test]
    fn almost_black_art_with_a_small_logo_takes_the_logo() {
        let img = image(|x, y| if (28..36).contains(&x) && (28..36).contains(&y) { [40, 200, 230] } else { [6, 6, 8] });
        let c = extract(img.into_iter()).unwrap();
        assert!(c[2] > c[0], "{c:?}");
        assert_usable(c);
    }

    #[test]
    fn colourless_art_has_no_accent() {
        assert_eq!(extract(image(|_, _| [3, 3, 3]).into_iter()), None);
        assert_eq!(extract(image(|_, _| [250, 250, 248]).into_iter()), None);
        assert_eq!(extract(image(|x, y| { let v = ((x + y) * 2) as u8; [v, v, v] }).into_iter()), None);
    }

    #[test]
    fn single_colour_neon_is_tamed() {
        for neon in [[255, 0, 255], [0, 255, 0], [0, 0, 255], [255, 255, 0], [255, 0, 0]] {
            let c = extract(image(|_, _| neon).into_iter()).unwrap();
            assert_usable(c);
            let [_, chroma, _] = to_lch(oklab(usable(c).0));
            assert!(chroma <= C_RANGE.1 + 0.001, "{neon:?} -> chroma {chroma}");
        }
    }

    #[test]
    fn dark_and_pale_colours_are_lifted_or_lowered_to_usable() {
        for rgb in [[40, 10, 60], [10, 30, 15], [250, 235, 240], [200, 230, 255], [120, 70, 20]] {
            assert_usable(rgb);
        }
    }

    #[test]
    fn a_palette_has_the_covers_hues_most_prominent_first() {
        // Three colours in different amounts, on black.
        let img = image(|x, y| match (x / 16, y / 32) {
            (0 | 1, _) => [220, 40, 50],
            (2, 0) => [40, 180, 230],
            (3, 0) => [250, 200, 40],
            _ => [5, 5, 5],
        });
        let p = palette(img.into_iter(), 3);
        assert_eq!(p.len(), 3, "{p:?}");
        let hue = |c: [u8; 3]| to_lch(oklab(c))[2];
        assert!((hue(p[0]) - hue([220, 40, 50])).abs() < 0.3, "red first: {p:?}");
        // Colourless art has none; one colour gives one.
        assert!(palette(image(|_, _| [128, 128, 128]).into_iter(), 3).is_empty());
        assert_eq!(palette(image(|_, _| [30, 90, 200]).into_iter(), 3).len(), 1);
        // Each is light and vivid enough to read on the dark background.
        for c in palette(image(|x, _| if x < 32 { [60, 20, 20] } else { [20, 40, 70] }).into_iter(), 3) {
            let [l, ch, _] = to_lch(oklab(c));
            assert!(l >= 0.54 && ch >= 0.05, "{c:?}");
        }
    }

    #[test]
    fn transition_keeps_its_colour_between_complementary_hues() {
        let (a, _) = usable([60, 120, 255]);
        let (b, _) = usable([255, 160, 40]);
        for i in 0..=10 {
            let m = mix(a, b, i as f32 / 10.0);
            let [_, c, _] = to_lch(oklab(m));
            assert!(c > 0.06, "step {i}: {m:?} chroma {c}");
        }
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
    }
}
