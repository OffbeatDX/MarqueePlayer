//! Drawing the visualizer: each style's geometry as path commands and its colours as
//! brushes, for one stage (the band behind Now playing, the panel's tab, the full page, the
//! Settings preview). No Slint state here, so each style's geometry is unit-tested;
//! `visualizer/mod.rs` hands the result to the `Visualizer` global, where a few `Path`s draw it
//! (`VisualizerStage` in vizstage.slint).
//!
//! The stage is split in two: the top box, where the bars rise from its bottom edge (the
//! baseline), and under it the reflection's box. Paths are in each box's logical pixels.
//!
//! - Bars: rounded bars, caps above them, a scaled mirror below.
//! - Line: a smooth curve (Catmull-Rom as cubic Béziers) over a filled area.
//! - Ring, around the round cover, in five kinds: Ripples (the spectrum as a glowing
//!   contour, the two halves mirrored; each beat sends the contour out as a ripple and
//!   throws sparks off it), Saturn (a tilted
//!   ring of dust in orbit, behind the cover and in front), Galaxy (two spiral arms of
//!   dust), Swarm (dust in a whirl, blown out by each beat) and Eclipse (flares arcing off
//!   the rim and back).
//! - Bonfire: the spectrum as a low glowing line along the foot, sparks rising from it.
//!
//! A level is shaped before it is drawn: above 0.8 it bends toward the top instead of
//! running into it (`shape`), so loud music keeps its contour rather than a flat roof.

use std::fmt::Write as _;

use slint::private_unstable_api::re_exports::{ConicGradientBrush, GradientStop, LinearGradientBrush, RadialGradientBrush};
use slint::{Brush, Color};

use crate::visualizer::look::{ColourBy, Colours, Detail, Look, Style};

/// Reflection strips for a gradient across (each its own alpha; see `Paths::refl`).
pub const STRIPS: usize = 6;

/// A level as drawn: straight up to 0.8, then easing toward 1 without reaching it.
pub fn shape(x: f32) -> f32 {
    let x = x.max(0.0);
    if x <= 0.8 { x } else { 0.8 + 0.2 * ((x - 0.8) / 0.2).tanh() }
}

/// What the stage is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stage {
    pub w: f32,
    pub h: f32,
    /// The bars' width and gap at Medium detail (theme.toml).
    pub bar: (f32, f32),
    /// The halo's hub: the cover it surrounds, its half size (its radius, round), in the
    /// stage's middle. The page lays it out (`VisualizerStage.hub` in vizstage.slint): around
    /// the page's own cover behind the page, a share of the stage elsewhere. 0: that share.
    pub hub: f32,
    /// The cover's corner radius when the halo follows its square shape; below 0: round.
    pub corner: f32,
}

impl Stage {
    pub fn new(w: f32, h: f32, bar: (f32, f32)) -> Stage {
        Stage { w, h, bar, hub: 0.0, corner: -1.0 }
    }
}

/// The paths of one frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paths {
    /// Bars, the line's area, the ring's corona (the light
    /// between the cover and its contour).
    pub fill: String,
    /// Peak caps.
    pub caps: String,
    /// The line, the ring's contour (drawn with its glow under it).
    pub line: String,
    /// The reflection: one path (a gradient up the bars, its fade in the brush), or
    /// `STRIPS` horizontal slices of it, each fainter (a gradient across).
    pub refl: [String; STRIPS],
    pub top_h: f32,
    pub refl_h: f32,
    /// The ring: how loud the low notes are now (0..1), for its bloom's brightness.
    pub pulse: f32,
    /// The ring: the last beat's strength, dying away (0..1), for the cover's own pulse.
    pub punch: f32,
    /// The ring's ripples: the contours that left the cover on a beat, on their way out.
    pub ripples: String,
    /// The ring's snare rings: thin, quick, gone well before the ripples' way ends.
    pub rings: String,
    /// Sparks (the ring's, the bonfire's): the young ones' dots, the old ones' (fainter),
    /// and the soft discs of light round them.
    pub sparks: String,
    pub sparks_old: String,
    pub sparks_glow: String,
    /// What is in front of the cover (Saturn's near grains, drawn over it), and the discs of
    /// light round the loud ones.
    pub front: String,
    pub front_glow: String,
}

/// How many bars (or points) a look has across a stage, and how many the analysis gives
/// (half of them when mirrored).
pub fn layout(look: &Look, stage: &Stage) -> (usize, usize) {
    let w = stage.w.max(1.0);
    let n = match look.style {
        Style::Bars => {
            let (bw, gap) = bar_size(look, stage);
            // At least 16 (a narrow panel): the bars get narrower rather than fewer. At
            // most 240: more cost more to draw than they show.
            (((w + gap) / (bw + gap)).floor() as usize).clamp(16, 240)
        }
        Style::Line | Style::Bonfire => ((w / point_spacing(look.detail)).floor() as usize + 1).clamp(16, 240),
        Style::Radial | Style::Saturn | Style::Galaxy | Style::Swarm | Style::Eclipse => {
            // Points around the halo's middle line, this far apart: a curve through them,
            // so a few dozen read as one shape.
            let pitch = match look.detail {
                Detail::Fine => 7.0,
                Detail::Medium => 11.0,
                Detail::Coarse => 16.0,
            };
            ((halo(stage).perimeter() / pitch) as usize / 2 * 2).clamp(48, 432)
        }
    };
    let mirrored = look.mirror && matches!(look.style, Style::Bars | Style::Line) || look.style.around();
    // One analysis band per 1/24 octave at most (the range is about nine octaves): finer
    // is noise, not detail.
    let bands = if mirrored { n.div_ceil(2) } else { n }.min(216);
    let n = if mirrored { bands * 2 } else { bands };
    (n, bands)
}

/// A bar's width and the gap between bars, as the detail asks.
fn bar_size(look: &Look, stage: &Stage) -> (f32, f32) {
    match (look.style, look.detail) {
        (_, Detail::Fine) => (4.0, 2.0),
        (_, Detail::Medium) => (stage.bar.0.clamp(1.0, 64.0), stage.bar.1.clamp(0.0, 64.0)),
        (_, Detail::Coarse) => (22.0, 6.0),
    }
}

// A curve's points: each is a cubic Bézier for Skia to flatten, stroke and fill every
// frame, so no more than the eye needs (measured on the full page: points 5 px apart cost
// a few percent of a core more than 12 px, which looks the same; straight segments sampled
// here instead of curves cost no less).
fn point_spacing(d: Detail) -> f32 {
    match d {
        Detail::Fine => 7.0,
        Detail::Medium => 12.0,
        Detail::Coarse => 18.0,
    }
}

/// The hub's share of the halo's stage when the stage does not say (half its smaller side,
/// less the edge): vizstage.slint's `VisualizerStage` lays the cover out the same way.
pub const HUB: f32 = 0.58;
/// What the halo keeps clear at the stage's edge.
const EDGE: f32 = 2.0;

/// The ring's shape on a stage: the hub (the cover) in the middle, the contour starting just
/// outside it and reaching out by the level, its ripples travelling on to the stage's edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Halo {
    pub cx: f32,
    pub cy: f32,
    /// The hub's half size (the cover's radius, round).
    pub hub: f32,
    /// Between the cover's edge and the contour's foot.
    pub gap: f32,
    /// The contour's longest reach from its foot.
    pub len: f32,
    /// How far from the foot a ripple travels before it is gone (a cover's radius: it may
    /// leave the stage on the way).
    pub travel: f32,
    /// The glow's width around the contour.
    pub glow: f32,
    /// The cover's corner radius when the ring follows a square cover; `None`: round.
    pub corner: Option<f32>,
}

pub fn halo(stage: &Stage) -> Halo {
    let half = stage.w.min(stage.h) / 2.0;
    let hub = if stage.hub > 0.0 { stage.hub.min(half - EDGE - 4.0) } else { (half - EDGE) * HUB }.max(6.0);
    let room = (half - EDGE - hub).max(4.0);
    let glow = (room * 0.14).clamp(3.0, 18.0);
    let gap = (hub * 0.02).clamp(1.5, 5.0);
    // The contour stays inside the stage, reaching out by half the cover's radius at most.
    // The ripples go on for a whole radius, past the stage's edge where that is nearer
    // (the stage clips them: they leave the page rather than stop short of its edge).
    let len = (room - gap - glow * 0.5 - 2.0).min(hub * 0.5).max(2.0);
    let travel = hub.max(len + 2.0);
    let corner = (stage.corner >= 0.0).then(|| stage.corner.min(hub));
    Halo { cx: stage.w / 2.0, cy: stage.h / 2.0, hub, gap, len, travel, glow, corner }
}

impl Halo {
    /// The band's foot, round or following the cover's rounded square: the length of its
    /// middle line (half the reach out), which the points are spread along.
    pub fn perimeter(&self) -> f32 {
        let mid = self.hub + self.gap + self.len * 0.5;
        match self.corner {
            None => std::f32::consts::TAU * mid,
            Some(c) => {
                let q = c + self.gap + self.len * 0.5;
                8.0 * (mid - q).max(0.0) + std::f32::consts::TAU * q
            }
        }
    }

    /// `n` points around the band's foot, from the top middle, clockwise: each its place on
    /// the foot and the way out (a unit vector). Round: evenly by angle. Square: evenly along
    /// the middle line, so the corners, where the way out turns, get their share.
    pub fn frame(&self, n: usize) -> Vec<((f32, f32), (f32, f32))> {
        let foot = self.hub + self.gap;
        match self.corner {
            None => (0..n)
                .map(|k| {
                    let a = (k as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
                    let d = (a.sin(), -a.cos());
                    ((self.cx + foot * d.0, self.cy + foot * d.1), d)
                })
                .collect(),
            Some(c) => {
                // The middle line: a rounded square of half size `m`, corners of radius `q`;
                // a point on it less the way out times the middle's offset is on the foot.
                let off = self.len * 0.5;
                let m = foot + off;
                let q = c + self.gap + off;
                let s = (m - q).max(0.0);
                let arc = std::f32::consts::FRAC_PI_2 * q;
                let side = 2.0 * s;
                let total = 4.0 * (side + arc);
                (0..n)
                    .map(|k| {
                        // From the top middle, clockwise: half a side, a corner, a side...
                        let mut t = ((k as f32 + 0.5) / n as f32 * total + s) % total;
                        let mut quarter = 0;
                        while t >= side + arc && quarter < 3 {
                            t -= side + arc;
                            quarter += 1;
                        }
                        // In the top side's frame (going right), then turned by the quarter.
                        let (p, d) = if t < side {
                            ((-s + t, -m), (0.0, -1.0))
                        } else {
                            let a = ((t - side) / q.max(1e-3)).min(std::f32::consts::FRAC_PI_2);
                            ((s + q * a.sin(), -s - q * a.cos()), (a.sin(), -a.cos()))
                        };
                        let turn = |v: (f32, f32)| match quarter {
                            0 => v,
                            1 => (-v.1, v.0),
                            2 => (-v.0, -v.1),
                            _ => (v.1, -v.0),
                        };
                        let (p, d) = (turn(p), turn(d));
                        ((self.cx + p.0 - d.0 * off, self.cy + p.1 - d.1 * off), d)
                    })
                    .collect()
            }
        }
    }
}

/// The top box and the reflection's, for a stage of height `h`.
pub fn split(look: &Look, h: f32, reflect_here: bool) -> (f32, f32) {
    if reflect_here && look.reflection && look.style.reflects() {
        let top = (h / (1.0 + look.reflection_size)).floor();
        (top, h - top)
    } else {
        (h, 0.0)
    }
}

/// What was heard of the beat this frame (`beat::Tracker`), for what moves with it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pulse {
    /// A beat to show: its strength (to 1), 0 for none.
    pub hit: f32,
    /// A snare, a clap: its strength (to 1), 0 for none. Sparks, no ripple.
    pub snap: f32,
    /// The beat's length in seconds, 0 while no tempo is known.
    pub period: f32,
    /// How bright the sound is now (`beat::Heard::bright`).
    pub bright: f32,
    /// One of the music's beats fell in this frame; where in the beat the frame is (0 at
    /// the beat, toward 1 before the next); and the low notes' onset over what is usual
    /// (`beat::Heard`).
    pub beat: bool,
    pub phase: f32,
    pub low: f32,
}

/// Draws frames, keeping what must carry over between them (the ring's ripples on their
/// way out, the sparks).
#[derive(Default)]
pub struct Draw {
    pub paths: Paths,
    /// The time since the last frame drawn (`tick`), for what moves by itself.
    dt: f32,
    ripples: Ripples,
    sparks: Sparks,
    saturn: Saturn,
    galaxy: Galaxy,
    swarm: Swarm,
    eclipse: Eclipse,
    /// The style the sparks (and the rest of what moves by itself) are of.
    sparks_of: Option<Style>,
    /// The beat as heard for the next frame (`pulse`); without it a frame listens to the
    /// levels it is given (a test, a caller with no sound to listen to).
    heard: Option<Pulse>,
}

impl Draw {
    /// This much time has passed since the last frame: the next `draw` moves the ripples
    /// on by it. A `draw` without it is a still frame (the Settings preview with nothing
    /// playing): the ripples as a moment of the music would leave them.
    pub fn tick(&mut self, dt: f32) {
        self.dt += dt.max(0.0);
    }

    /// The beat as heard since the last frame: the next `draw` moves with it (the ring's
    /// ripples and sparks, the bonfire's).
    pub fn pulse(&mut self, heard: Pulse) {
        self.heard = Some(heard);
    }

    /// Nothing drawn and nothing carried over (the visualizer stopped).
    pub fn reset(&mut self) {
        self.paths = Paths::default();
        self.dt = 0.0;
        self.ripples = Ripples::default();
        self.sparks = Sparks::default();
        self.saturn = Saturn::default();
        self.galaxy = Galaxy::default();
        self.swarm = Swarm::default();
        self.eclipse = Eclipse::default();
        self.sparks_of = None;
        self.heard = None;
    }

    /// The paths for these levels and peaks (0..1, `bands` of them).
    pub fn draw(&mut self, look: &Look, stage: &Stage, reflect_here: bool, levels: &[f32], peaks: &[f32]) -> &Paths {
        let p = &mut self.paths;
        p.fill.clear();
        p.caps.clear();
        p.line.clear();
        p.refl.iter_mut().for_each(String::clear);
        (p.pulse, p.punch) = (0.0, 0.0);
        p.ripples.clear();
        p.rings.clear();
        p.sparks.clear();
        p.sparks_old.clear();
        p.sparks_glow.clear();
        p.front.clear();
        p.front_glow.clear();
        let (top, refl) = split(look, stage.h, reflect_here);
        p.top_h = top;
        p.refl_h = refl;
        let strips = look.colour_by == ColourBy::Frequency && !matches!(look.colours, Colours::Accent);
        let dt = std::mem::take(&mut self.dt);
        let heard = self.heard.take();
        if look.style != Style::Radial {
            self.ripples = Ripples::default();
        }
        // Another style's sparks are not this one's (the ring's are measured round it, the
        // bonfire's across the page).
        if self.sparks_of != Some(look.style) {
            self.sparks = Sparks::default();
            self.saturn = Saturn::default();
            self.galaxy = Galaxy::default();
            self.swarm = Swarm::default();
            self.eclipse = Eclipse::default();
            self.sparks_of = look.style.beats().then_some(look.style);
        }
        match look.style {
            Style::Bars => bars(p, look, stage, levels, peaks, strips),
            Style::Line => line(p, look, stage, levels, peaks, strips),
            Style::Radial => radial(p, &mut self.ripples, &mut self.sparks, (dt, heard), look, stage, levels),
            Style::Bonfire => bonfire(p, &mut self.sparks, (dt, heard), look, stage, levels),
            Style::Saturn => saturn(p, &mut self.saturn, (dt, heard), look, stage, levels),
            Style::Galaxy => galaxy(p, &mut self.galaxy, (dt, heard), look, stage, levels),
            Style::Swarm => swarm(p, &mut self.swarm, (dt, heard), look, stage, levels),
            Style::Eclipse => eclipse(p, &mut self.eclipse, (dt, heard), look, stage, levels),
        }
        &self.paths
    }
}

/// Bands in drawing order: as analysed, or (mirrored) the highest first, down to the
/// lowest in the middle, and up again.
fn order(bands: usize, mirror: bool) -> impl Iterator<Item = usize> {
    let n = if mirror { bands * 2 } else { bands };
    (0..n).map(move |i| if !mirror { i } else if i < bands { bands - 1 - i } else { i - bands })
}

fn get(v: &[f32], i: usize) -> f32 {
    shape(v.get(i).copied().unwrap_or(0.0))
}

/// A rectangle from y `a` down to `b` (a < b), clipped to the strip `[s0, s1)`.
fn strip_rect(out: &mut String, x: f32, w: f32, a: f32, b: f32, s0: f32, s1: f32) {
    let (a, b) = (a.max(s0), b.min(s1));
    if b - a >= 0.25 {
        let _ = write!(out, "M{x:.1} {a:.1}H{:.1}V{b:.1}H{x:.1}Z", x + w);
    }
}

/// The reflection's strips: their bounds in the reflection box.
fn strip_bounds(refl_h: f32, k: usize) -> (f32, f32) {
    (refl_h * k as f32 / STRIPS as f32, refl_h * (k + 1) as f32 / STRIPS as f32)
}

fn bars(p: &mut Paths, look: &Look, stage: &Stage, levels: &[f32], peaks: &[f32], strips: bool) {
    let (n, bands) = layout(look, stage);
    let mirror = look.mirror;
    let (bw0, gap) = bar_size(look, stage);
    let bw = fit_bars(stage.w, n, bw0, gap);
    let used = n as f32 * (bw + gap) - gap;
    let x0 = ((stage.w - used) / 2.0).max(0.0);
    let h = p.top_h;
    let room = (h - 4.0).max(1.0);
    // Softly rounded tops on bars wide enough to show them.
    let r = if bw >= 8.0 { (bw * 0.35).min(2.5) } else { 0.0 };
    for (slot, i) in order(bands, mirror).enumerate() {
        // Whole pixels: the bars are drawn without anti-aliasing (`crisp`), much cheaper
        // for hundreds of them, and an edge on a pixel needs none.
        let xa = (x0 + slot as f32 * (bw + gap)).round();
        let (x, bw) = (xa, ((x0 + slot as f32 * (bw + gap) + bw).round() - xa).max(1.0));
        let l = get(levels, i);
        let bh = (l * room).round();
        if bh >= 1.0 {
            let top = h - bh;
            if bh > r * 2.0 && r > 0.4 {
                let _ = write!(
                    p.fill,
                    "M{x:.1} {h:.1}V{:.1}Q{x:.1} {top:.1} {:.1} {top:.1}H{:.1}Q{:.1} {top:.1} {:.1} {:.1}V{h:.1}Z",
                    top + r,
                    x + r,
                    x + bw - r,
                    x + bw,
                    x + bw,
                    top + r
                );
            } else {
                let _ = write!(p.fill, "M{x:.1} {h:.1}V{top:.1}H{:.1}V{h:.1}Z", x + bw);
            }
            if p.refl_h > 0.0 {
                // A 1 px gap at the baseline reads as the surface the bars stand on.
                let rh = (l * p.refl_h).round();
                if strips {
                    for k in 0..STRIPS {
                        let (s0, s1) = strip_bounds(p.refl_h, k);
                        strip_rect(&mut p.refl[k], x, bw, 1.0, rh, s0, s1);
                    }
                } else if rh >= 1.5 {
                    let _ = write!(p.refl[0], "M{x:.1} 1H{:.1}V{rh:.1}H{x:.1}Z", x + bw);
                }
            }
        }
        let pk = get(peaks, i);
        // A cap only where there is something to cap: 2 px tall, just above the bar.
        if look.peaks && pk * room >= 8.0 {
            let y = (h - pk * room - 3.0).max(0.0).round();
            let _ = write!(p.caps, "M{x:.1} {y:.1}H{:.1}V{:.1}H{x:.1}Z", x + bw, y + 2.0);
        }
    }
}

/// A bar's width for `n` bars: as asked when they fit that way, else what fills the width
/// (fewer bands than fit at that size: wider; the fewest allowed on a narrow stage:
/// narrower).
fn fit_bars(w: f32, n: usize, bw: f32, gap: f32) -> f32 {
    let fit = (((w + gap) / (bw + gap)).floor() as usize).max(1);
    if n == fit { bw } else { ((w + gap) / n.max(1) as f32 - gap).max(1.0) }
}

/// A smooth curve through points (Catmull-Rom, as cubic Béziers), its control points kept
/// within `lo..hi` so it never dips under the baseline. Continues `out` (no move-to).
fn curve(out: &mut String, pts: &[(f32, f32)], lo: f32, hi: f32) {
    for i in 0..pts.len().saturating_sub(1) {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, (p1.1 + (p2.1 - p0.1) / 6.0).clamp(lo, hi));
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, (p2.1 - (p3.1 - p1.1) / 6.0).clamp(lo, hi));
        let _ = write!(out, "C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}", c1.0, c1.1, c2.0, c2.1, p2.0, p2.1);
    }
}

fn line(p: &mut Paths, look: &Look, stage: &Stage, levels: &[f32], peaks: &[f32], strips: bool) {
    let (n, bands) = layout(look, stage);
    let h = p.top_h;
    let room = (h - 3.0).max(1.0);
    let w = stage.w;
    let dx = w / (n.max(2) - 1) as f32;
    // A light smoothing across neighbours: the line reads as one shape, not a comb.
    let raw: Vec<f32> = order(bands, look.mirror).map(|i| get(levels, i)).collect();
    let smooth = |v: &[f32], k: usize| {
        let a = v[k.saturating_sub(1)];
        let c = v[(k + 1).min(v.len() - 1)];
        0.25 * a + 0.5 * v[k] + 0.25 * c
    };
    let lv: Vec<f32> = (0..raw.len()).map(|k| smooth(&raw, k)).collect();
    if lv.iter().all(|&l| l * room < 0.5) {
        return;
    }
    let pts: Vec<(f32, f32)> = lv.iter().enumerate().map(|(k, &l)| (k as f32 * dx, h - l * room)).collect();
    let _ = write!(p.line, "M{:.1} {:.1}", pts[0].0, pts[0].1);
    curve(&mut p.line, &pts, 0.0, h);
    let _ = write!(p.fill, "M0 {h:.1}L{:.1} {:.1}", pts[0].0, pts[0].1);
    curve(&mut p.fill, &pts, 0.0, h);
    let _ = write!(p.fill, "L{w:.1} {h:.1}Z");
    if look.peaks {
        for (k, i) in order(bands, look.mirror).enumerate() {
            let pk = get(peaks, i);
            if pk * room >= 8.0 && pk > lv[k] + 0.02 {
                let (x, y) = (k as f32 * dx, h - pk * room - 2.0);
                let _ = write!(p.caps, "M{:.1} {y:.1}H{:.1}V{:.1}H{:.1}Z", x - 1.5, x + 1.5, y + 2.0, x - 1.5);
            }
        }
    }
    if p.refl_h > 0.0 {
        let rh = p.refl_h;
        if strips {
            // Each strip: the area under the mirrored line, clamped to the strip.
            for k in 0..STRIPS {
                let (s0, s1) = strip_bounds(rh, k);
                if lv.iter().all(|&l| l * rh < s0 + 0.25) {
                    continue;
                }
                let _ = write!(p.refl[k], "M0 {s0:.1}");
                for (j, &l) in lv.iter().enumerate() {
                    let _ = write!(p.refl[k], "L{:.1} {:.1}", j as f32 * dx, (l * rh).clamp(s0, s1));
                }
                let _ = write!(p.refl[k], "L{w:.1} {s0:.1}Z");
            }
        } else {
            let rp: Vec<(f32, f32)> = lv.iter().enumerate().map(|(k, &l)| (k as f32 * dx, (l * rh).max(1.0))).collect();
            let _ = write!(p.refl[0], "M0 1L{:.1} {:.1}", rp[0].0, rp[0].1);
            curve(&mut p.refl[0], &rp, 1.0, rh);
            let _ = write!(p.refl[0], "L{w:.1} 1Z");
        }
    }
}

/// Values around a closed ring smoothed with their neighbours (a quarter each side), `passes`
/// times: the halo reads as one moving shape, not a comb.
fn smooth_ring(v: &mut Vec<f32>, passes: usize) {
    let n = v.len();
    if n < 3 {
        return;
    }
    let mut tmp = vec![0.0; n];
    for _ in 0..passes {
        for k in 0..n {
            tmp[k] = 0.25 * v[(k + n - 1) % n] + 0.5 * v[k] + 0.25 * v[(k + 1) % n];
        }
        std::mem::swap(v, &mut tmp);
    }
}

/// A smooth closed curve through points (Catmull-Rom, as cubic Béziers), from the first
/// round to it again.
fn closed_curve(out: &mut String, pts: &[(f32, f32)]) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    let _ = write!(out, "M{:.1} {:.1}", pts[0].0, pts[0].1);
    for i in 0..n {
        let p0 = pts[(i + n - 1) % n];
        let (p1, p2) = (pts[i], pts[(i + 1) % n]);
        let p3 = pts[(i + 2) % n];
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        let _ = write!(out, "C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}", c1.0, c1.1, c2.0, c2.1, p2.0, p2.1);
    }
    out.push('Z');
}

/// The longest a ripple's way out takes, from leaving the contour to where it is gone
/// (seconds), and its speed without a tempo to go by (pixels a second): with less room it is
/// gone sooner, not slower.
const RIPPLE_LIFE: f32 = 3.2;
const RIPPLE_SPEED: f32 = 95.0;
/// The most on their way at once, where there is room for them (`ripple_room`).
const RIPPLE_MAX: usize = 5;
/// The least room a ripple has to itself, on average (pixels of the way out).
const RIPPLE_PITCH: f32 = 36.0;
/// The least time between two, however fast the kicks.
const RIPPLE_GAP: f32 = 0.16;
/// Without kicks: the least time between two that leave on a swell, and how long the music
/// may go without one before one leaves anyway (long enough not to be heard as a clock).
const RIPPLE_REST: f32 = 0.46;
const RIPPLE_LULL: f32 = 2.4;
/// While the music has no beat to go by, a hit weaker than this is not taken for a kick
/// (music without any has its low notes' wavering told as weak ones, a few a second).
const RIPPLE_SURE: f32 = 0.55;
/// With a beat to go by, what sends a ripple depends on where in the beat it is. A kick is
/// told this long after it started (seconds); it is on the beat, or on the half beat,
/// within this share of a beat. On the beat: any kick told, or an onset of the low notes
/// this many times the usual (`GRID_LOW.0`: a kick the tracker missed under a bass line,
/// a bass note with the beat; never much before the beat, `GRID_EARLY`). On the half beat: a firm kick (`GRID_HIT`) that the low
/// notes bear out (`GRID_LOW.1`). Off both: nothing. Measured over the steady excerpts of
/// a real library (3,964 beats): of the kicks told, 57% were on the beat, 19% on the half
/// beat and 24% off both, as strong off the beat as on it (a bass line's notes, which
/// fall anywhere: they are what made the ripples leave at random); the low notes had an
/// onset of 3 times the usual on three beats in four.
const RIPPLE_LATE: f32 = 0.025;
const GRID_NEAR: f32 = 0.14;
const GRID_LOW: (f32, f32) = (3.0, 3.0);
/// An onset of the low notes sends its ripple no sooner than this share of a beat before
/// the beat (the low notes waver: one that only leads up to the kick is not it).
const GRID_EARLY: f32 = 0.05;
const GRID_HIT: f32 = 0.75;
/// How a ripple's way out is made up. The throw: the share of the way the kick that sent
/// it throws it (the hardest kick's; a soft one's is 0.6 of that), dying away over this
/// share of the way's time, and how long after it left the low notes still say how hard the
/// kick was (seconds: a kick is told before its peak). The push: the share of the way every
/// later kick pushes it (a full one), and how fast a push dies away (per second). The rest
/// of the way it drifts, so that it is gone when the way's time is up.
const RIPPLE_THROW: f32 = 0.55;
const RIPPLE_THROW_TIME: f32 = 0.3;
const RIPPLE_WINDUP: f32 = 0.15;
const RIPPLE_PUSH: f32 = 0.1;
const RIPPLE_PUSH_RATE: f32 = 6.0;
/// How far round a ripple turns on its way out (radians): they swirl a little.
const RIPPLE_TWIST: f32 = 0.34;
/// A ripple's shape is the contour's, this much more pronounced (an echo that reads as one
/// from a distance, where it is faint).
const RIPPLE_SHAPE: f32 = 1.5;
/// Where the ripples of a still frame are on their way.
const RIPPLE_STILL: [f32; 6] = [0.84, 0.68, 0.52, 0.38, 0.24, 0.1];
/// A snare's ring: how far of the ripples' way it goes (all of it, in two beats:
/// it overtakes them; its brush is gone there), the most at once and the least time between
/// two. A snap is any sharp edge: a snare's, a clap's, but also a kick's own, a plucked
/// note's. What tells them apart is whether the sound is still bright a moment later (a
/// snare's noise lasts, a kick's edge is over in milliseconds, a note was never bright): a
/// ring is shown once it is `RING_HOLD` old (two frames), if the sound since it was told
/// has been at least `RING_CRACK` bright.
const RING_REACH: f32 = 1.0;
const RING_MAX: usize = 4;
const RING_GAP: f32 = 0.1;
const RING_HOLD: f32 = 0.03;
const RING_CRACK: f32 = 0.07;
/// How fast a spark slows (per second): the ring's, thrown outward; the bonfire's, rising.
const SPARK_DRAG: f32 = 1.1;
const FIRE_DRAG: f32 = 0.9;

/// How many ripples a ring has room for at once (a small stage's would be crowded by as
/// many as the full page's; and each is a long curve to draw, most of what this style
/// costs), and how long the way out takes without a tempo to go by (the same speed whatever
/// the room).
fn ripple_room(g: &Halo) -> (usize, f32) {
    let most = ((g.travel / RIPPLE_PITCH) as usize).clamp(3, RIPPLE_MAX);
    (most, (g.travel / RIPPLE_SPEED).clamp(1.2, RIPPLE_LIFE))
}

/// What of the ripples' ways is left out, for comparing (`MP_HALO_WITHOUT`, words separated
/// by commas: `throw`, `push`, `swells`). Those are the design. The snares' rings are not:
/// tried on real music on 2026-11-25, a ring could not be connected to any sound heard as
/// it appeared, and they are off unless `MP_HALO_WITH=rings` asks for them (kept for another
/// try at tuning them).
#[derive(Clone, Copy)]
struct Tune {
    /// A kick throws its ripple, harder the harder it is; without: every ripple drifts.
    throw: bool,
    /// Every kick pushes the ripples on their way.
    push: bool,
    /// A snare sends a ring (off unless asked for).
    rings: bool,
    /// Without kicks a ripple leaves on a swell of the music; without: after each rest.
    swells: bool,
}

fn tune() -> Tune {
    static TUNE: std::sync::OnceLock<Tune> = std::sync::OnceLock::new();
    *TUNE.get_or_init(|| {
        let listed = |var: &str, word: &str| std::env::var(var).unwrap_or_default().split(',').any(|w| w.trim().eq_ignore_ascii_case(word));
        let on = |word: &str| !listed("MP_HALO_WITHOUT", word);
        // The rings are off unless asked for (`MP_HALO_WITH=rings`), and on in the tests,
        // which are about them too.
        Tune { throw: on("throw"), push: on("push"), rings: (cfg!(test) && on("rings")) || listed("MP_HALO_WITH", "rings"), swells: on("swells") }
    })
}

/// A beat, found in a level over time: a rise above where it has been, told when the rise
/// tops out (what leaves on a beat carries the moment at its fullest).
#[derive(Default)]
struct Beat {
    /// The level followed slowly.
    slow: f32,
    /// Last frame's level, and whether a rise is under way.
    was: f32,
    rising: bool,
    /// A frame has been seen (the first sets the levels to follow from, and is no beat).
    started: bool,
}

impl Beat {
    /// This frame's level: whether a rise has just topped out.
    fn step(&mut self, level: f32, dt: f32) -> bool {
        if !self.started {
            (self.started, self.slow, self.was) = (true, level, level);
        }
        if level - self.slow > 0.06 && level > self.was {
            self.rising = true;
        }
        let topped = self.rising && level <= self.was;
        if topped {
            self.rising = false;
        }
        self.slow += (level - self.slow) * (1.0 - (-dt / 0.4).exp());
        self.was = level;
        topped
    }
}

/// The ring's ripples between frames.
struct Ripples {
    list: Vec<Ripple>,
    rings: Vec<Ring>,
    /// Since the last ripple left, since the last ring did, and since the last beat.
    since: f32,
    ring_since: f32,
    quiet: f32,
    /// How far apart the kicks' ripples have lately left (seconds; 0: not known yet), for
    /// the way's time while no tempo is known.
    pace: f32,
    /// A ripple waiting for the next beat to leave on (how hard it will be thrown): a
    /// swell's or a lull's, where the music has a beat but no kicks.
    due: Option<f32>,
    beat: Beat,
    /// The last beat's strength, dying away in an eighth of a second: the contour jumps
    /// with each kick, and the bloom with it.
    punch: f32,
}

impl Default for Ripples {
    fn default() -> Self {
        Ripples { list: Vec::new(), rings: Vec::new(), since: 0.0, ring_since: 0.0, quiet: 0.0, pace: 0.0, due: None, beat: Beat::default(), punch: 0.0 }
    }
}

struct Ripple {
    /// The contour as it left (0..1 of the contour's reach, around the ring), and the same
    /// smoothed: it relaxes toward that as it travels, as a wave on water does.
    shape: Vec<f32>,
    soft: Vec<f32>,
    /// How far out it is (0..1 of its way), and for how long it has been going.
    far: f32,
    age: f32,
    /// The throw that sent it: the share of the way it is good for, the speed left of it
    /// (ways a second) and the time that dies away in.
    throw: f32,
    v: f32,
    tau: f32,
    /// The speed left of the kicks' pushes since (ways a second).
    pushed: f32,
    /// Making room for a newer one: on its way out quickly.
    leaving: bool,
}

impl Ripple {
    /// The contour as it is, `far` along its way; `hit`: the beat that sent it (a harder
    /// one sends a larger ripple).
    fn new(mut shape: Vec<f32>, far: f32, hit: f32) -> Ripple {
        let mean = shape.iter().sum::<f32>() / shape.len().max(1) as f32;
        shape.iter_mut().for_each(|v| *v = ((mean + (*v - mean) * RIPPLE_SHAPE) * (1.0 + 0.3 * hit)).max(0.0));
        let mut soft = shape.clone();
        smooth_ring(&mut soft, 6);
        let mean = soft.iter().sum::<f32>() / soft.len().max(1) as f32;
        soft.iter_mut().for_each(|v| *v = mean + (*v - mean) * 0.75);
        Ripple { shape, soft, far, age: 0.0, throw: 0.0, v: 0.0, tau: 1.0, pushed: 0.0, leaving: false }
    }

    /// Thrown `throw` of the way (more than it was: the kick turned out harder).
    fn thrown(&mut self, throw: f32) {
        self.v += (throw - self.throw).max(0.0) / self.tau;
        self.throw = self.throw.max(throw);
    }
}

/// A snare's ring between frames.
struct Ring {
    /// The contour as it left, smoothed (0..1 of the contour's reach, around the ring).
    soft: Vec<f32>,
    age: f32,
    life: f32,
    leaving: bool,
    /// Whether it shows: not before it is known to be a snare's (`RING_CRACK`); until then,
    /// how bright the sound has been since it was told, summed over the frames.
    shown: bool,
    bright: f32,
    frames: f32,
}

impl Ring {
    /// How far out it is (0..1 of the ripples' way): quickly out and slowing, to where its
    /// brush is gone.
    fn far(&self) -> f32 {
        let u = (self.age / self.life).clamp(0.0, 1.0);
        RING_REACH * (1.0 - (-u / 0.5).exp()) / (1.0 - (-2.0f32).exp())
    }
}

/// Sparks between frames: the ring's, thrown off its contour, and the bonfire's, rising
/// from its coals.
#[derive(Default)]
struct Sparks {
    list: Vec<Spark>,
    /// Sparks owed by the steady trickle (the fraction carried from frame to frame).
    owed: f32,
    seed: u32,
    beat: Beat,
}

struct Spark {
    /// Where it left from: round the ring (0..1 from the top, clockwise), or across the page.
    at: f32,
    /// How far it has come (pixels) and how fast it is going (pixels a second).
    far: f32,
    v: f32,
    age: f32,
    life: f32,
    size: f32,
    /// Its sway's phase (the bonfire's drift from side to side).
    sway: f32,
}

/// What throwing sparks needs to know.
struct Throw<'a> {
    /// The level where a spark would leave from (0..1): more leave from the loud places.
    level: &'a dyn Fn(f32) -> f32,
    /// How far out it starts there (pixels).
    from: &'a dyn Fn(f32) -> f32,
    /// A slow spark's speed, and the most a fast one adds (pixels a second).
    speed: (f32, f32),
    /// The shortest and longest life (seconds), the smallest and largest size (pixels).
    life: (f32, f32),
    size: (f32, f32),
    /// The most at once.
    most: usize,
    /// The steady trickle (a second, at full level) and a beat's burst.
    rate: f32,
    burst: usize,
}

impl Sparks {
    /// The next of a fixed sequence, 0..1 (the same every run: a still frame is the same
    /// each time, and so is a test).
    fn rand(&mut self) -> f32 {
        if self.seed == 0 {
            self.seed = 0x9e37_79b9;
        }
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// `count` more, `kick` times as fast as usual (a beat's burst), while there is room.
    fn throw(&mut self, count: usize, kick: f32, t: &Throw) {
        for _ in 0..count {
            if self.list.len() >= t.most {
                return;
            }
            // Where: a few tries for a loud place, then wherever the last try fell.
            let mut at = self.rand();
            for _ in 0..6 {
                if self.rand() <= (t.level)(at) * 1.4 + 0.1 {
                    break;
                }
                at = self.rand();
            }
            let l = (t.level)(at).clamp(0.0, 1.0);
            let v = (t.speed.0 + t.speed.1 * self.rand() * (0.5 + l)) * kick;
            let life = t.life.0 + (t.life.1 - t.life.0) * self.rand();
            let size = t.size.0 + (t.size.1 - t.size.0) * self.rand();
            let sway = self.rand() * std::f32::consts::TAU;
            self.list.push(Spark { at, far: (t.from)(at), v, age: 0.0, life, size, sway });
        }
    }

    /// Time passes: each moves on, slowing, and those at their life's end go.
    fn step(&mut self, dt: f32, drag: f32) {
        let slow = (-dt * drag).exp();
        for s in &mut self.list {
            s.age += dt;
            s.far += s.v * dt;
            s.v *= slow;
        }
        self.list.retain(|s| s.age < s.life);
    }

    /// A frame of them. With time passed: the old ones move on, a trickle joins them, and a
    /// beat (`hit`: its strength, 0 for none) throws a burst, larger and faster the harder
    /// it is; a snare or a clap (`snap`) a smaller one. With none: a still frame, a third
    /// of the most, each somewhere on its way.
    fn frame(&mut self, dt: f32, drag: f32, sound: f32, (hit, snap): (f32, f32), t: &Throw) {
        if dt > 0.0 {
            self.step(dt, drag);
            if sound > 0.03 {
                self.owed += dt * t.rate * sound;
                let now = self.owed as usize;
                self.owed -= now as f32;
                self.throw(now, 1.0, t);
                if hit > 0.0 {
                    self.throw((t.burst as f32 * (0.5 + hit)) as usize, 1.15 + 0.5 * hit, t);
                }
                if snap > 0.0 {
                    self.throw((t.burst as f32 * (0.2 + 0.4 * snap)) as usize, 1.0 + 0.3 * snap, t);
                }
            }
        } else {
            *self = Sparks::default();
            if sound > 0.03 {
                self.throw(t.most / 3, 1.0, t);
                for i in 0..self.list.len() {
                    let u = self.rand();
                    let s = &mut self.list[i];
                    s.age = u * s.life * 0.9;
                    s.far += s.v / drag * (1.0 - (-drag * s.age).exp());
                }
            }
        }
    }

    /// Each as a dot, smaller as it ages (an ember going out), with a soft disc of light
    /// round it: the young ones' dots, the old ones' (fainter: their own path and brush),
    /// and the discs. `place` says where a spark is, or that it has left the stage.
    fn draw(&self, p: &mut Paths, place: &dyn Fn(&Spark) -> Option<(f32, f32)>) {
        for s in &self.list {
            let u = s.age / s.life;
            let r = s.size * (1.0 - u).powf(0.7);
            if r < 0.3 {
                continue;
            }
            let Some((x, y)) = place(s) else { continue };
            dot(if u < 0.45 { &mut p.sparks } else { &mut p.sparks_old }, x, y, r);
            // The young ones only: a disc is the larger shape to fill, and an old spark's
            // would hardly show.
            if u < 0.6 {
                dot(&mut p.sparks_glow, x, y, r * 2.6);
            }
        }
    }
}

/// A filled circle.
fn dot(out: &mut String, x: f32, y: f32, r: f32) {
    let _ = write!(out, "M{:.1} {y:.1}A{r:.1} {r:.1} 0 1 0 {:.1} {y:.1}A{r:.1} {r:.1} 0 1 0 {:.1} {y:.1}Z", x - r, x + r, x - r);
}

/// The ring: the cover as a drum skin. The spectrum is a contour around it, glowing, the
/// light between it and the cover its corona: the low notes swell it all round (how loud
/// they are now is also the bloom's brightness), the rest of the spectrum shapes it, from the
/// low mids at the top to the highs at the bottom, the two halves mirrored (no seam where the
/// highs meet at the bottom, nor at the top); the look's intensity is how far it swings. On
/// each beat the contour as it is then leaves the cover as a ripple, travelling out a
/// cover's radius, flattening and fading as it goes, and sparks are thrown off the contour
/// where it is loudest; a few leave it all the time (`dt` moves them all on; with none, a
/// still frame: a few as a moment would leave them). The beat is the one heard (`heard`:
/// each kick is a jump of the whole contour, a ripple and a burst, harder for a harder one),
/// or without that a rise in the levels. Every kick has its ripple (one too many on their
/// way: the oldest hurries out); a kick throws its ripple, faster the harder it is, and
/// pushes those already on their way, which drift between kicks, so the whole field moves
/// with the beat and takes as many beats over the way as there is room for ripples. A
/// snare sends a thin quick ring. Without kicks a ripple leaves when the music swells (a
/// rise in the levels topping out), not by the clock. Its foot follows the cover, round or
/// square.
fn radial(p: &mut Paths, rip: &mut Ripples, sparks: &mut Sparks, (dt, heard): (f32, Option<Pulse>), look: &Look, stage: &Stage, levels: &[f32]) {
    let (n, bands) = layout(look, stage);
    let g = halo(stage);
    let frame = g.frame(n);
    // The lowest eighth of the bands: the swell; the others around the ring.
    let low = (bands / 8).clamp(1, bands.saturating_sub(1).max(1));
    let half = n / 2;
    let swell = (0..low).map(|i| get(levels, i)).sum::<f32>() / low as f32;
    let intensity = look.intensity.clamp(0.5, 2.0);
    let mut lv: Vec<f32> = (0..n)
        .map(|k| {
            let j = if k < half { k } else { n - 1 - k };
            // Band (fractional) for this point of the half ring: the low mids at the top.
            let b = low as f32 + j as f32 / (half.max(2) - 1) as f32 * (bands - 1 - low) as f32;
            let (i0, t) = (b.floor().max(0.0) as usize, b - b.floor());
            let l = get(levels, i0) * (1.0 - t) + get(levels, (i0 + 1).min(bands - 1)) * t;
            (0.25 * swell + 0.75 * l).max(0.0)
        })
        .collect();
    let energy = lv.iter().sum::<f32>() / n.max(1) as f32;
    // The beat: as heard, or (nobody listening) a rise in the low notes and a little of
    // everything, told when it tops out.
    // A rise in the levels topping out is followed whoever listens: without kicks it is what
    // sends a ripple.
    let level = 0.7 * swell + 0.3 * energy;
    let rise = (level - rip.beat.slow).max(0.0);
    let topped = dt > 0.0 && rip.beat.step(level, dt);
    let (hit, snap, period, bright) = match heard {
        _ if dt <= 0.0 => (0.0, 0.0, 0.0, 0.0),
        Some(h) => (h.hit.clamp(0.0, 1.0), h.snap.clamp(0.0, 1.0), h.period, h.bright),
        None => (if topped { 0.7 } else { 0.0 }, 0.0, 0.0, 0.0),
    };
    let (phase, onset, on_beat) = heard.map_or((0.0, 0.0, false), |h| (h.phase, h.low, h.beat));
    rip.punch = if dt > 0.0 { (rip.punch * (-dt / 0.13).exp()).max(hit) } else { 0.0 };
    // The kick's jump on top, all round; then bent toward the contour's reach, never past
    // it (`shape`).
    lv.iter_mut().for_each(|l| *l = shape((*l + 0.2 * rip.punch) * intensity));
    smooth_ring(&mut lv, 1);
    (p.pulse, p.punch) = (swell.max(rip.punch), rip.punch);
    // The contour. At rest it lies on its foot: a thin lit line around the cover.
    let edge: Vec<(f32, f32)> = frame.iter().zip(&lv).map(|(&((x, y), (dx, dy)), &l)| (x + dx * l * g.len, y + dy * l * g.len)).collect();
    closed_curve(&mut p.line, &edge);
    // The corona: between the cover and the contour, one path with the cover's place left
    // out of it (the contour clockwise, the foot back the other way), so nothing is painted
    // under the cover.
    if lv.iter().any(|&l| l * g.len >= 1.0) {
        closed_curve(&mut p.fill, &edge);
        match g.corner {
            None => {
                let f = g.hub + g.gap;
                let _ = write!(p.fill, "M{:.1} {:.1}A{f:.1} {f:.1} 0 1 0 {:.1} {:.1}A{f:.1} {f:.1} 0 1 0 {:.1} {:.1}Z", g.cx, g.cy - f, g.cx, g.cy + f, g.cx, g.cy - f);
            }
            Some(_) => {
                let foot: Vec<(f32, f32)> = frame.iter().rev().map(|&(q, _)| q).collect();
                closed_curve(&mut p.fill, &foot);
            }
        }
    }
    // The sparks: off the contour, where it is loudest, out past where the ripples end.
    {
        let foot = g.hub + g.gap;
        let level = |a: f32| {
            let f = a.rem_euclid(1.0) * n as f32 - 0.5 + n as f32;
            let (i0, t) = (f.floor() as usize, f - f.floor());
            lv[i0 % n] * (1.0 - t) + lv[(i0 + 1) % n] * t
        };
        let from = |a: f32| level(a) * g.len;
        // As big as the cover makes them (a spark by a small cover is a small spark), and as
        // many as the way out has room for.
        let size = ((g.hub * 0.006).clamp(0.9, 2.0), (g.hub * 0.017).clamp(2.0, 5.0));
        let most = ((g.travel * 0.9) as usize).clamp(60, 240);
        let throw = Throw { level: &level, from: &from, speed: (g.hub * 0.32, g.hub * 0.8), life: (1.0, 2.2), size, most, rate: most as f32 * 0.4, burst: most / 6 };
        sparks.frame(dt, SPARK_DRAG, energy, (hit, snap), &throw);
        let end = foot + g.travel * 1.15;
        sparks.draw(p, &|s| {
            let (th, r) = (s.at * std::f32::consts::TAU, foot + s.far);
            (r <= end).then(|| (g.cx + th.sin() * r, g.cy - th.cos() * r))
        });
    }
    // The ripples: a third as many points as the contour (they are smoother than it, and
    // their cost is their curves: measured, they are most of what this style costs to draw).
    let m = (n / 3).max(3);
    // The contour's level at each of `k` points round it (between two of its own).
    let sample = |k: usize| -> Vec<f32> {
        (0..k)
            .map(|j| {
                let at = (j as f32 + 0.5) / k as f32 * n as f32 - 0.5;
                let (i0, t) = (at.floor().max(0.0) as usize, at - at.floor());
                lv[i0.min(n - 1)] * (1.0 - t) + lv[(i0 + 1) % n] * t
            })
            .collect()
    };
    // A ring is rounder still: half the ripples' points.
    let mr = (m / 2).max(12);
    let tune = tune();
    let (most, life) = ripple_room(&g);
    let lane = life / most as f32;
    if dt > 0.0 {
        // Where in the beat what is told now started: how far past the beat (before it:
        // negative), and from the half beat.
        let at = if period > 0.0 { (phase - RIPPLE_LATE / period).rem_euclid(1.0) } else { 0.0 };
        let (past, to_half) = (if at < 0.5 { at } else { at - 1.0 }, (at - 0.5).abs());
        // What is a kick to the ripples. Without a beat to go by: a hit that is surely one.
        // With one: it depends on where in the beat it is.
        let hit = if period <= 0.0 {
            if hit >= RIPPLE_SURE { hit } else { 0.0 }
        } else if past.abs() <= GRID_NEAR {
            // An onset of the low notes with no kick told: a soft one, harder the stronger.
            if hit > 0.0 {
                hit
            } else if onset >= GRID_LOW.0 && past >= -GRID_EARLY {
                (0.3 + 0.08 * (onset - GRID_LOW.0)).min(0.7)
            } else {
                0.0
            }
        } else if to_half <= GRID_NEAR && hit >= GRID_HIT && onset >= GRID_LOW.1 {
            hit
        } else {
            0.0
        };
        // The kick itself told a moment after its onset sent the ripple: the ripple is the
        // kick's, thrown as hard.
        if hit > 0.0
            && rip.since < 0.08
            && let Some(r) = rip.list.last_mut()
        {
            r.thrown(if tune.throw { RIPPLE_THROW * (0.6 + 0.4 * (0.5 * hit).max(swell.clamp(0.0, 1.0))) } else { 0.0 });
        }
        rip.quiet = if hit > 0.0 { 0.0 } else { rip.quiet + dt };
        // No kick for more than a beat and a half: a breakdown, an intro, music without any.
        let beatless = rip.quiet > if period > 0.0 { 1.6 * period } else { 1.0 };
        if beatless || rip.pace <= 0.0 {
            rip.pace = lane;
        }
        // A beat's length: the tempo's, or as far apart as the kicks' ripples have left.
        let beat = if period > 0.0 { period } else { rip.pace };
        // The way out takes as many beats as there is room for ripples: one leaves on each
        // kick, so those on their way are a beat apart. Without kicks, as long as it takes
        // at the ripples' own speed.
        let way = if beatless { life } else { (most as f32 * beat).clamp(1.0, if period > 0.0 { RIPPLE_LIFE } else { life }) };
        // What the kicks to come will push a ripple, of its way (so the drift makes up the
        // rest, and it is gone when the way's time is up).
        let pushes = if tune.push && !beatless { (RIPPLE_PUSH * 0.75 * (way / beat - 1.0).max(0.0)).min(0.3) } else { 0.0 };
        let push = if tune.push && hit > 0.0 { RIPPLE_PUSH * RIPPLE_PUSH_RATE * (0.4 + 0.6 * hit) } else { 0.0 };
        let low = swell.clamp(0.0, 1.0);
        let throw = |power: f32| if tune.throw { RIPPLE_THROW * (0.6 + 0.4 * power) } else { 0.0 };
        for r in &mut rip.list {
            r.age += dt;
            // A kick is told before its peak: how loud the low notes get in its first
            // moments says how hard it was.
            if r.age < RIPPLE_WINDUP {
                r.thrown(throw(low));
            }
            r.pushed += push;
            let (e, f) = ((-dt / r.tau).exp(), (-dt * RIPPLE_PUSH_RATE).exp());
            let drift = (1.0 - r.throw - pushes).max(0.12) / way;
            r.far += r.v * r.tau * (1.0 - e) + r.pushed * (1.0 - f) / RIPPLE_PUSH_RATE + (drift + if r.leaving { 3.0 } else { 0.0 }) * dt;
            r.v *= e;
            r.pushed *= f;
        }
        rip.list.retain(|r| r.far < 1.0 && r.shape.len() == m);
        rip.since += dt;
        // The rings: a snare's, a clap's. Every sharp edge is a snap and has its ring, kept
        // back for two frames: shown if the sound has stayed bright since (`RING_CRACK`),
        // dropped unseen if not (a kick's own edge, a plucked note).
        rip.ring_since += dt;
        let mut shown = false;
        rip.rings.retain_mut(|r| {
            r.age += if r.leaving { 4.0 * dt } else { dt };
            if r.shown {
                return r.age < r.life && r.soft.len() == mr;
            }
            r.bright += bright;
            r.frames += 1.0;
            if r.age >= RING_HOLD {
                r.shown = r.bright / r.frames >= RING_CRACK;
                shown |= r.shown;
                return r.shown;
            }
            true
        });
        // Never more than there is room for: the oldest makes room.
        if shown
            && rip.rings.iter().filter(|r| r.shown && !r.leaving).count() > RING_MAX
            && let Some(old) = rip.rings.iter_mut().find(|r| r.shown && !r.leaving)
        {
            old.leaving = true;
        }
        // With a beat to go by, a ring is a snare's or a clap's on the beat: an open hat on
        // every half beat is bright too, and three rings a second were the hats'.
        if tune.rings && snap > 0.0 && energy > 0.03 && (period <= 0.0 || past.abs() <= GRID_NEAR) && (rip.ring_since >= (0.42 * beat).clamp(RING_GAP, 0.3) || rip.rings.is_empty()) {
            let mut soft = sample(mr);
            smooth_ring(&mut soft, 3);
            rip.rings.push(Ring { soft, age: 0.0, life: if period > 0.0 { (2.0 * period).clamp(0.6, 1.2) } else { 0.9 }, leaving: false, shown: false, bright: 0.0, frames: 0.0 });
            rip.ring_since = 0.0;
        }
        // What sends a ripple now, and how hard: a kick (every one that is not right on the
        // last one's heels); without kicks a swell of the music, or a long lull.
        let send = if energy <= 0.03 {
            None
        } else if hit > 0.0 {
            (rip.since >= (0.42 * beat).clamp(RIPPLE_GAP, 0.3) || rip.list.is_empty()).then_some((0.5 * hit).max(low))
        } else if !beatless {
            None
        } else if !tune.swells {
            (rip.since >= (lane * 1.5).max(RIPPLE_REST)).then_some(0.0)
        } else if period > 0.0 {
            // No kicks, but the music keeps a beat: a swell's ripple, or a lull's, waits
            // for the next beat and leaves on it.
            if topped && rip.since >= RIPPLE_REST {
                rip.due = Some((0.3 + 2.0 * rise).min(0.8).max(rip.due.unwrap_or(0.0)));
            } else if rip.since >= RIPPLE_LULL && rip.due.is_none() {
                rip.due = Some(0.0);
            }
            if on_beat { rip.due.take() } else { None }
        } else if topped && rip.since >= RIPPLE_REST {
            Some((0.3 + 2.0 * rise).min(0.8))
        } else {
            (rip.since >= RIPPLE_LULL).then_some(0.0)
        };
        if hit > 0.0 || period <= 0.0 {
            rip.due = None;
        }
        if let Some(power) = send {
            if hit > 0.0 {
                rip.pace += (rip.since.min(lane) - rip.pace) * 0.35;
            }
            // Never more than there is room for: the oldest makes room, out at the edge
            // where it is all but gone.
            if rip.list.iter().filter(|r| !r.leaving).count() >= most
                && let Some(old) = rip.list.iter_mut().find(|r| !r.leaving)
            {
                old.leaving = true;
            }
            let mut r = Ripple::new(sample(m), 0.0, hit);
            r.tau = RIPPLE_THROW_TIME * way;
            r.thrown(throw(power));
            rip.list.push(r);
            rip.since = 0.0;
        }
    } else {
        *rip = Ripples::default();
        if energy > 0.03 {
            // As many as there is room for, spread over the way out.
            let k = most.min(RIPPLE_STILL.len());
            rip.list = (0..k).map(|i| Ripple::new(sample(m), RIPPLE_STILL[i * RIPPLE_STILL.len() / k], 0.0)).collect();
        }
    }
    let around = g.frame(m);
    let mut pts = Vec::with_capacity(m);
    for r in &rip.list {
        // Its shape kept, lower and softer the farther it is: an echo of the contour. It
        // fades with the distance (the brush).
        let u = r.far.clamp(0.0, 1.0);
        let out = g.travel * u;
        let amp = g.len * (1.0 - u);
        let relax = (u * 1.25).min(1.0);
        // Turned round as it goes: the shape read a little back along the ring.
        let shift = RIPPLE_TWIST * u / std::f32::consts::TAU * m as f32;
        let (whole, part) = (shift.floor() as usize, shift - shift.floor());
        let turned = |v: &[f32], j: usize| {
            let a = (j + m - whole % m) % m;
            v[a] * (1.0 - part) + v[(a + m - 1) % m] * part
        };
        pts.clear();
        pts.extend(around.iter().enumerate().map(|(j, &((x, y), (dx, dy)))| {
            let (s, soft) = (turned(&r.shape, j), turned(&r.soft, j));
            let d = ((s + (soft - s) * relax) * amp + out).min(g.travel);
            (x + dx * d, y + dy * d)
        }));
        closed_curve(&mut p.ripples, &pts);
    }
    // The rings: off the contour (smoothed), quickly out and slowing, to where their brush
    // is gone.
    let around = g.frame(mr);
    for r in rip.rings.iter().filter(|r| r.shown) {
        let u = (r.age / r.life).clamp(0.0, 1.0);
        let out = g.travel * r.far();
        let amp = g.len * (1.0 - u);
        pts.clear();
        pts.extend(around.iter().zip(&r.soft).map(|(&((x, y), (dx, dy)), &s)| {
            let d = (s * amp + out).min(g.travel);
            (x + dx * d, y + dy * d)
        }));
        closed_curve(&mut p.rings, &pts);
    }
}

/// The bonfire: the spectrum as a low glowing line along the stage's foot (its coals: low
/// notes on the left, a sixth of the height at most), and sparks rising from it where the
/// music is loudest, slowing and swaying as they go, each going out as it ages; a few all
/// the time, a burst on every beat, the one heard or a rise in the levels (`dt` moves them
/// on; with none, a still frame).
fn bonfire(p: &mut Paths, sparks: &mut Sparks, (dt, heard): (f32, Option<Pulse>), look: &Look, stage: &Stage, levels: &[f32]) {
    let (n, bands) = layout(look, stage);
    let (w, h) = (stage.w, p.top_h);
    let dx = w / (n.max(2) - 1) as f32;
    let raw: Vec<f32> = (0..n).map(|k| get(levels, k.min(bands.saturating_sub(1)))).collect();
    // A light smoothing across neighbours: the coals read as one shape, not a comb.
    let lv: Vec<f32> = (0..n).map(|k| 0.25 * raw[k.saturating_sub(1)] + 0.5 * raw[k] + 0.25 * raw[(k + 1).min(n - 1)]).collect();
    let coals = (h * 0.16).clamp(10.0, 110.0).min((h - 3.0).max(1.0));
    let foot = h - 2.0;
    if lv.iter().any(|&l| l * coals >= 0.5) {
        let pts: Vec<(f32, f32)> = lv.iter().enumerate().map(|(k, &l)| (k as f32 * dx, foot - shape(l) * coals)).collect();
        let _ = write!(p.line, "M{:.1} {:.1}", pts[0].0, pts[0].1);
        curve(&mut p.line, &pts, 0.0, foot);
        let _ = write!(p.fill, "M0 {h:.1}L{:.1} {:.1}", pts[0].0, pts[0].1);
        curve(&mut p.fill, &pts, 0.0, foot);
        let _ = write!(p.fill, "L{w:.1} {h:.1}Z");
    }
    let energy = lv.iter().sum::<f32>() / n.max(1) as f32;
    let low = (n / 8).max(1);
    let swell = lv[..low.min(n)].iter().sum::<f32>() / low as f32;
    let level = |x: f32| {
        let f = x.clamp(0.0, 1.0) * (n - 1) as f32;
        let (i0, t) = (f.floor() as usize, f - f.floor());
        lv[i0.min(n - 1)] * (1.0 - t) + lv[(i0 + 1).min(n - 1)] * t
    };
    let from = |x: f32| 2.0 + shape(level(x)) * coals;
    let big = (h / 500.0).clamp(0.7, 1.3);
    let most = ((w * h / 2600.0) as usize).clamp(60, 420);
    let throw = Throw { level: &level, from: &from, speed: (h * 0.16, h * 0.55), life: (1.2, 2.8), size: (0.9 * big, 2.7 * big), most, rate: most as f32 * 0.42, burst: most / 8 };
    let beat = match heard {
        _ if dt <= 0.0 => (0.0, 0.0),
        Some(h) => (h.hit.clamp(0.0, 1.0), h.snap.clamp(0.0, 1.0)),
        None => (if sparks.beat.step(0.7 * swell + 0.3 * energy, dt) { 0.7 } else { 0.0 }, 0.0),
    };
    sparks.frame(dt, FIRE_DRAG, energy, beat, &throw);
    sparks.draw(p, &|s| {
        let y = h - s.far;
        (y > -6.0).then(|| (s.at * w + (s.sway + s.age * 2.2).sin() * 5.0, y))
    });
}

/// A spark's dot and, while it is young enough to show one, its disc of light: `u` is how
/// far through its life it is (the young ones' dots, the old ones', the discs: three paths).
fn spot(p: &mut Paths, x: f32, y: f32, r: f32, u: f32) {
    if r < 0.3 {
        return;
    }
    dot(if u < 0.45 { &mut p.sparks } else { &mut p.sparks_old }, x, y, r);
    if u < 0.6 {
        dot(&mut p.sparks_glow, x, y, r * 2.6);
    }
}

/// The next of a fixed sequence, 0..1 (the same every run).
fn rand(seed: &mut u32) -> f32 {
    if *seed == 0 {
        *seed = 0x9e37_79b9;
    }
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    (*seed >> 8) as f32 / (1u32 << 24) as f32
}

/// The beat for a look this frame: as heard (a kick's strength, a snap's), or, nobody
/// listening, a rise in `level` told when it tops out; nothing in a still frame.
fn beat_of(heard: Option<Pulse>, own: &mut Beat, level: f32, dt: f32) -> (f32, f32, f32) {
    match heard {
        _ if dt <= 0.0 => (0.0, 0.0, 0.0),
        Some(h) => (h.hit.clamp(0.0, 1.0), h.snap.clamp(0.0, 1.0), h.period),
        None => (if own.step(level, dt) { 0.7 } else { 0.0 }, 0.0, 0.0),
    }
}

/// Saturn's ring: how flat it lies (its short axis over its long one) and how far it is
/// turned (radians).
const SATURN_TILT: f32 = 0.3;
const SATURN_TURN: f32 = -0.32;

/// Saturn's dust between frames.
#[derive(Default)]
struct Saturn {
    grains: Vec<Grain>,
    /// The last beat's strength, dying away: the ring swells and hurries with it.
    push: f32,
    beat: Beat,
}

struct Grain {
    /// Where on its orbit (radians), how far out (in cover's radii), how big, how fast.
    th: f32,
    r: f32,
    size: f32,
    k: f32,
}

/// Saturn: a tilted ring of dust in orbit round the cover, the grains on its far side
/// behind the cover (`sparks_old`, drawn under it) and those on the near side in front
/// (`front`, drawn over it). Each grain is as big as its note is loud (the spectrum round
/// the ring, as the ring style has it) and the loud ones have a disc of light; the inner
/// grains go round faster than the outer; the bass makes the whole ring swell, and a beat
/// pushes it out and hurries it for a moment.
fn saturn(p: &mut Paths, st: &mut Saturn, (dt, heard): (f32, Option<Pulse>), look: &Look, stage: &Stage, levels: &[f32]) {
    let (n, bands) = layout(look, stage);
    let g = halo(stage);
    let low = (bands / 8).clamp(1, bands.saturating_sub(1).max(1));
    let half = n / 2;
    let swell = (0..low).map(|i| get(levels, i)).sum::<f32>() / low as f32;
    let lv: Vec<f32> = (0..n)
        .map(|k| {
            let j = if k < half { k } else { n - 1 - k };
            let b = low as f32 + j as f32 / (half.max(2) - 1) as f32 * (bands - 1 - low) as f32;
            let (i0, t) = (b.floor().max(0.0) as usize, b - b.floor());
            get(levels, i0) * (1.0 - t) + get(levels, (i0 + 1).min(bands - 1)) * t
        })
        .collect();
    let energy = (lv.iter().sum::<f32>() / n.max(1) as f32).max(swell * 0.5);
    let (hit, _, _) = beat_of(heard, &mut st.beat, 0.7 * swell + 0.3 * energy, dt);
    st.push = if dt > 0.0 { (st.push * (-dt * 3.5).exp()).max(hit) } else { 0.0 };
    (p.pulse, p.punch) = (swell.max(st.push), st.push);
    // As many grains as the cover is big, the same ones every run.
    let most = ((g.hub * 1.1) as usize).clamp(90, 300);
    if st.grains.len() != most {
        let mut seed = 0x51ed_270b;
        st.grains = (0..most)
            .map(|_| Grain { th: rand(&mut seed) * std::f32::consts::TAU, r: 1.28 + 0.6 * rand(&mut seed).powf(0.8), size: 0.7 + 1.5 * rand(&mut seed), k: 0.7 + 0.6 * rand(&mut seed) })
            .collect();
    }
    if dt > 0.0 {
        let hurry = 1.0 + 1.4 * swell + 1.2 * st.push;
        for q in &mut st.grains {
            q.th = (q.th + dt * 0.45 * q.k * q.r.powf(-1.5) * hurry) % std::f32::consts::TAU;
        }
    }
    // Nothing to hear: the dust fades out (and silence draws nothing).
    let shown = (energy / 0.05).min(1.0);
    if shown < 0.02 {
        return;
    }
    // A thin lit line round the cover.
    let rim: Vec<(f32, f32)> = g.frame(48.max(n / 4)).iter().map(|&(q, _)| q).collect();
    closed_curve(&mut p.line, &rim);
    let (cs, sn) = (SATURN_TURN.cos(), SATURN_TURN.sin());
    let big = (g.hub * 0.0065).clamp(0.8, 1.9) * shown;
    let out = 1.0 + 0.07 * st.push + 0.05 * swell;
    for q in &st.grains {
        let a = (q.th / std::f32::consts::TAU).rem_euclid(1.0) * n as f32;
        let (i0, t) = (a.floor() as usize % n, a - a.floor());
        let l = (lv[i0] * (1.0 - t) + lv[(i0 + 1) % n] * t).min(1.0);
        let rr = g.hub * q.r * out;
        let (ex, ey) = (q.th.cos() * rr, q.th.sin() * rr * SATURN_TILT);
        let (x, y) = (g.cx + ex * cs - ey * sn, g.cy + ex * sn + ey * cs);
        // The far half of the orbit is behind the cover.
        if q.th.sin() < 0.0 {
            dot(&mut p.sparks_old, x, y, q.size * big * (0.7 + 1.0 * l) * 0.85);
        } else {
            let r = q.size * big * (0.7 + 1.0 * l) * 1.12;
            dot(&mut p.front, x, y, r);
            // The loud ones only, and close about them: a wide flat disc reads as a bubble.
            if l > 0.45 {
                dot(&mut p.front_glow, x, y, r * 1.9);
            }
        }
    }
}

struct Mote {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    age: f32,
    life: f32,
    size: f32,
}

/// The spectrum round the cover, for the looks drawn around it: the cover's place on the
/// stage, the level at each of `n` points round it (the low mids at the top, the highs at
/// the bottom, both halves alike, a quarter of it the low notes' swell), that swell, and
/// how much sound there is at all.
struct Round {
    g: Halo,
    n: usize,
    lv: Vec<f32>,
    swell: f32,
    energy: f32,
}

impl Round {
    fn of(look: &Look, stage: &Stage, levels: &[f32]) -> Round {
        let (n, bands) = layout(look, stage);
        let low = (bands / 8).clamp(1, bands.saturating_sub(1).max(1));
        let half = n / 2;
        let swell = (0..low).map(|i| get(levels, i)).sum::<f32>() / low as f32;
        let lv: Vec<f32> = (0..n)
            .map(|k| {
                let j = if k < half { k } else { n - 1 - k };
                let b = low as f32 + j as f32 / (half.max(2) - 1) as f32 * (bands - 1 - low) as f32;
                let (i0, t) = (b.floor().max(0.0) as usize, b - b.floor());
                (0.25 * swell + 0.75 * (get(levels, i0) * (1.0 - t) + get(levels, (i0 + 1).min(bands - 1)) * t)).max(0.0)
            })
            .collect();
        let energy = lv.iter().sum::<f32>() / n.max(1) as f32;
        Round { g: halo(stage), n, lv, swell, energy }
    }

    /// The level at a place round the cover (0..1 from the top, clockwise).
    fn at(&self, a: f32) -> f32 {
        let f = a.rem_euclid(1.0) * self.n as f32 - 0.5 + self.n as f32;
        let (i0, t) = (f.floor() as usize, f - f.floor());
        self.lv[i0 % self.n] * (1.0 - t) + self.lv[(i0 + 1) % self.n] * t
    }

    /// A thin contour round the cover: the spectrum, reaching out `reach` at the most,
    /// jumping with the beat (`punch`).
    fn contour(&self, out: &mut String, reach: f32, punch: f32) {
        let mut lv: Vec<f32> = self.lv.iter().map(|&l| shape(l + 0.2 * punch)).collect();
        smooth_ring(&mut lv, 1);
        let edge: Vec<(f32, f32)> = self.g.frame(self.n).iter().zip(&lv).map(|(&((x, y), (dx, dy)), &l)| (x + dx * l * reach, y + dy * l * reach)).collect();
        closed_curve(out, &edge);
    }
}

/// A grain of dust lit by its note: the loud ones bright with a disc of light close about
/// them, the quiet ones dim (their own path and brush).
fn lit(p: &mut Paths, x: f32, y: f32, r: f32, l: f32) {
    if l > 0.4 {
        dot(&mut p.sparks, x, y, r);
        if l > 0.6 {
            dot(&mut p.sparks_glow, x, y, r * 1.9);
        }
    } else {
        dot(&mut p.sparks_old, x, y, r);
    }
}

/// The galaxy between frames.
#[derive(Default)]
struct Galaxy {
    stars: Vec<Star>,
    turn: f32,
    push: f32,
    beat: Beat,
}

struct Star {
    /// How far along its arm (0 by the cover, 1 at the arm's end), which arm (its angle),
    /// how far off the arm's line and off its distance, how big.
    u: f32,
    arm: f32,
    off: f32,
    jit: f32,
    size: f32,
}

/// The galaxy: two spiral arms of dust turning slowly round the cover, seen from above.
/// Each grain is lit by its note (the low notes by the cover, the high ones at the arms'
/// ends); the bass winds the arms tighter and turns them faster, and a beat hurries them.
fn galaxy(p: &mut Paths, st: &mut Galaxy, (dt, heard): (f32, Option<Pulse>), look: &Look, stage: &Stage, levels: &[f32]) {
    let r = Round::of(look, stage, levels);
    let g = r.g;
    let (_, bands) = layout(look, stage);
    let (hit, _, _) = beat_of(heard, &mut st.beat, 0.7 * r.swell + 0.3 * r.energy, dt);
    st.push = if dt > 0.0 { (st.push * (-dt * 3.0).exp()).max(hit) } else { 0.0 };
    (p.pulse, p.punch) = (r.swell.max(st.push), st.push);
    let most = ((g.hub * 1.5) as usize).clamp(140, 420);
    if st.stars.len() != most {
        let mut seed = 0x6a09_e667;
        st.stars = (0..most)
            .map(|i| {
                let u = rand(&mut seed).powf(0.7);
                Star { u, arm: if i % 2 == 0 { 0.0 } else { std::f32::consts::PI }, off: (rand(&mut seed) - 0.5) * (0.28 + 0.5 * u), jit: (rand(&mut seed) - 0.5) * 0.06, size: 0.6 + 1.6 * rand(&mut seed) }
            })
            .collect();
    }
    if dt > 0.0 {
        st.turn = (st.turn + dt * (0.18 + 0.5 * r.swell + 0.4 * st.push)) % std::f32::consts::TAU;
    }
    let shown = (r.energy / 0.05).min(1.0);
    if shown < 0.02 {
        return;
    }
    let rim: Vec<(f32, f32)> = g.frame(48.max(r.n / 4)).iter().map(|&(q, _)| q).collect();
    closed_curve(&mut p.line, &rim);
    let big = (g.hub * 0.011).clamp(0.9, 3.0) * shown;
    let wind = 4.2 + 0.8 * r.swell;
    for s in &st.stars {
        let f = s.u * (bands.max(2) - 1) as f32;
        let (i0, t) = (f.floor() as usize, f - f.floor());
        let l = (get(levels, i0) * (1.0 - t) + get(levels, (i0 + 1).min(bands - 1)) * t).min(1.0);
        let d = g.hub * (1.1 + 0.85 * s.u + s.jit) * (1.0 + 0.05 * st.push);
        let th = st.turn + s.arm + s.u * wind + s.off;
        lit(p, g.cx + th.cos() * d, g.cy + th.sin() * d * 0.86, s.size * big * (0.6 + 1.3 * l), l);
    }
}

/// The swarm between frames.
#[derive(Default)]
struct Swarm {
    bees: Vec<Bee>,
    seed: u32,
    push: f32,
    beat: Beat,
}

struct Bee {
    /// Where it is and how it moves, in cover's radii from the cover's middle.
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// How far from the middle it keeps, how big it is, and its note (0..1 up the spectrum).
    home: f32,
    size: f32,
    note: f32,
}

/// The swarm: dust caught in a whirl round the cover. It circles faster the louder the
/// music is; every beat blows it outward, and each grain spirals back to the distance it
/// keeps. Each is lit by its own note.
fn swarm(p: &mut Paths, st: &mut Swarm, (dt, heard): (f32, Option<Pulse>), look: &Look, stage: &Stage, levels: &[f32]) {
    let r = Round::of(look, stage, levels);
    let g = r.g;
    let (_, bands) = layout(look, stage);
    let (hit, _, _) = beat_of(heard, &mut st.beat, 0.7 * r.swell + 0.3 * r.energy, dt);
    st.push = if dt > 0.0 { (st.push * (-dt * 3.0).exp()).max(hit) } else { 0.0 };
    (p.pulse, p.punch) = (r.swell.max(st.push), st.push);
    let most = ((g.hub * 1.3) as usize).clamp(120, 360);
    if st.bees.len() != most {
        st.seed = 0xbb67_ae85;
        st.bees = (0..most)
            .map(|_| {
                let (a, home) = (rand(&mut st.seed) * std::f32::consts::TAU, 1.15 + 0.75 * rand(&mut st.seed));
                Bee { x: a.cos() * home, y: a.sin() * home, vx: 0.0, vy: 0.0, home, size: 0.7 + 1.5 * rand(&mut st.seed), note: rand(&mut st.seed) }
            })
            .collect();
    }
    if dt > 0.0 {
        let spin = 0.5 + 2.2 * r.energy + 1.2 * r.swell;
        // In steps of a sixtieth at the most: the pull back is a spring, and a long frame
        // would overshoot it.
        let steps = ((dt * 60.0).ceil() as usize).clamp(1, 6);
        let h = dt / steps as f32;
        for step in 0..steps {
            for b in &mut st.bees {
                let d = (b.x * b.x + b.y * b.y).sqrt().max(0.05);
                let (nx, ny) = (b.x / d, b.y / d);
                if step == 0 && hit > 0.0 {
                    let k = (0.5 + 0.8 * rand(&mut st.seed)) * (0.5 + hit);
                    b.vx += nx * k;
                    b.vy += ny * k;
                }
                // Round the cover, and back toward the distance it keeps.
                let want = spin / d.sqrt();
                let back = (b.home - d) * 9.0;
                b.vx += ((-ny * want - b.vx) * 2.4 + nx * back) * h;
                b.vy += ((nx * want - b.vy) * 2.4 + ny * back) * h;
                b.x += b.vx * h;
                b.y += b.vy * h;
            }
        }
    }
    let shown = (r.energy / 0.05).min(1.0);
    if shown < 0.02 {
        return;
    }
    let rim: Vec<(f32, f32)> = g.frame(48.max(r.n / 4)).iter().map(|&(q, _)| q).collect();
    closed_curve(&mut p.line, &rim);
    let big = (g.hub * 0.011).clamp(0.9, 3.0) * shown;
    for b in &st.bees {
        let f = b.note * (bands.max(2) - 1) as f32;
        let (i0, t) = (f.floor() as usize, f - f.floor());
        let l = (get(levels, i0) * (1.0 - t) + get(levels, (i0 + 1).min(bands - 1)) * t).min(1.0);
        lit(p, g.cx + b.x * g.hub, g.cy + b.y * g.hub, b.size * big * (0.6 + 1.3 * l), l);
    }
}

/// The eclipse between frames.
#[derive(Default)]
struct Eclipse {
    motes: Vec<Mote>,
    seed: u32,
    /// Since the last flare.
    quiet: f32,
    punch: f32,
    beat: Beat,
}

/// The eclipse: the cover as an eclipsed sun. A glow at its rim (the bloom, brighter with
/// the low notes) and a thin contour; on each beat a flare of sparks leaves the rim where
/// the music is loudest, leaning one way, and the cover pulls it back in an arc. A harder
/// beat throws a second one; with sound and no beat for a while, a small one.
fn eclipse(p: &mut Paths, st: &mut Eclipse, (dt, heard): (f32, Option<Pulse>), look: &Look, stage: &Stage, levels: &[f32]) {
    let r = Round::of(look, stage, levels);
    let g = r.g;
    let (hit, _, _) = beat_of(heard, &mut st.beat, 0.7 * r.swell + 0.3 * r.energy, dt);
    st.punch = if dt > 0.0 { (st.punch * (-dt / 0.13).exp()).max(hit) } else { 0.0 };
    (p.pulse, p.punch) = (r.swell.max(st.punch), st.punch);
    let foot = g.hub + g.gap;
    let most = ((g.hub * 2.2) as usize).clamp(200, 600);
    let big = (g.hub * 0.011).clamp(0.9, 3.0);
    let flare = |st: &mut Eclipse, strength: f32| {
        // Where: a few tries for a loud place.
        let mut a = rand(&mut st.seed);
        for _ in 0..8 {
            if rand(&mut st.seed) <= r.at(a) * 1.5 + 0.08 {
                break;
            }
            a = rand(&mut st.seed);
        }
        let th = a * std::f32::consts::TAU;
        let lean = if rand(&mut st.seed) < 0.5 { -1.0 } else { 1.0 } * (0.5 + 0.5 * rand(&mut st.seed));
        for _ in 0..most / 8 {
            if st.motes.len() >= most {
                break;
            }
            let v = g.hub * (0.92 + 1.25 * rand(&mut st.seed)) * strength;
            let d = th + (rand(&mut st.seed) - 0.5) * 0.5;
            st.motes.push(Mote {
                x: g.cx + d.sin() * foot,
                y: g.cy - d.cos() * foot,
                vx: d.sin() * v + th.cos() * v * 0.5 * lean,
                vy: -d.cos() * v + th.sin() * v * 0.5 * lean,
                age: 0.0,
                life: 1.1 + 0.9 * rand(&mut st.seed),
                size: (0.7 + 1.7 * rand(&mut st.seed)) * big,
            });
        }
    };
    let step = |st: &mut Eclipse, h: f32| {
        for q in &mut st.motes {
            q.age += h;
            let (dx, dy) = (q.x - g.cx, q.y - g.cy);
            let d = (dx * dx + dy * dy).sqrt().max(1.0);
            // Pulled back toward the cover: the arc of a flare.
            let pull = g.hub * 3.96 / (d / g.hub).max(0.7);
            q.vx -= dx / d * pull * h;
            q.vy -= dy / d * pull * h;
            q.x += q.vx * h;
            q.y += q.vy * h;
        }
        st.motes.retain(|q| q.age < q.life && !(q.age > 0.3 && ((q.x - g.cx).powi(2) + (q.y - g.cy).powi(2)).sqrt() < g.hub));
    };
    if dt > 0.0 {
        step(st, dt);
        st.quiet += dt;
        if r.energy > 0.03 {
            if hit > 0.0 {
                flare(st, 0.8 + 0.6 * hit);
                if hit > 0.8 {
                    flare(st, 0.7);
                }
                st.quiet = 0.0;
            } else if st.quiet > 1.5 && r.energy > 0.05 {
                flare(st, 0.6);
                st.quiet = 0.0;
            }
        }
    } else {
        // A still frame: two flares a moment after they left.
        *st = Eclipse::default();
        if r.energy > 0.03 {
            flare(st, 1.1);
            flare(st, 0.9);
            for _ in 0..14 {
                step(st, 0.02);
            }
        }
    }
    if r.energy < 0.01 && st.motes.is_empty() {
        return;
    }
    r.contour(&mut p.line, g.len * 0.3, st.punch);
    for q in &st.motes {
        let u = q.age / q.life;
        spot(p, q.x, q.y, q.size * (1.0 - u).powf(0.7), u);
    }
}

// ---- Colours ----------------------------------------------------------------------------

/// The base gradient of a look, bottom (or left, or inside) first: (position, rgb, alpha).
pub fn base_stops(look: &Look, accent: [u8; 3], palette: &[[u8; 3]]) -> Vec<(f32, [u8; 3], f32)> {
    let even = |cs: &[[u8; 3]]| -> Vec<(f32, [u8; 3], f32)> {
        let n = cs.len().max(2) - 1;
        cs.iter().enumerate().map(|(i, &c)| (i as f32 / n as f32, c, 1.0)).collect()
    };
    match look.colours {
        // One colour: depth instead, fainter at the foot (the Classic look).
        Colours::Accent => vec![(0.0, accent, 0.5), (1.0, accent, 1.0)],
        Colours::Cover => {
            if palette.len() >= 2 {
                even(palette)
            } else {
                even(&analogous(palette.first().copied().unwrap_or(accent)))
            }
        }
        Colours::Custom => {
            let cs: Vec<[u8; 3]> = look.custom.iter().filter_map(|h| crate::visualizer::look::parse_hex(h)).collect();
            if cs.is_empty() { vec![(0.0, accent, 1.0), (1.0, accent, 1.0)] } else if cs.len() == 1 { even(&[cs[0], cs[0]]) } else { even(&cs) }
        }
        c => c.stops().unwrap_or(&[]).iter().map(|&(p, rgb)| (p, rgb, 1.0)).collect(),
    }
}

/// Three colours around one: its hue turned a little each way, darker below and lighter
/// above (a cover of one colour still gets a gradient).
pub fn analogous(c: [u8; 3]) -> [[u8; 3]; 3] {
    use library::accent::{from_lch, oklab, oklab_to_srgb, to_lch};
    let [l, ch, h] = to_lch(oklab(c));
    let ch = ch.max(0.08);
    let at = |dl: f32, dh: f32| oklab_to_srgb(from_lch([(l + dl).clamp(0.35, 0.92), ch, h + dh]));
    [at(-0.12, -0.55), c, at(0.08, 0.5)]
}

/// The colour of a gradient at `t` (0..1), with its alpha.
pub fn sample(stops: &[(f32, [u8; 3], f32)], t: f32) -> ([u8; 3], f32) {
    let t = t.clamp(0.0, 1.0);
    let Some(first) = stops.first() else { return ([255, 255, 255], 1.0) };
    if t <= first.0 {
        return (first.1, first.2);
    }
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.0 {
            let k = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 1.0 };
            let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * k).round() as u8;
            return ([mix(a.1[0], b.1[0]), mix(a.1[1], b.1[1]), mix(a.1[2], b.1[2])], a.2 + (b.2 - a.2) * k);
        }
    }
    let last = stops[stops.len() - 1];
    (last.1, last.2)
}

fn colour(rgb: [u8; 3], a: f32) -> Color {
    Color::from_argb_u8((a.clamp(0.0, 1.0) * 255.0).round() as u8, rgb[0], rgb[1], rgb[2])
}

fn stop(t: f32, rgb: [u8; 3], a: f32) -> GradientStop {
    GradientStop { color: colour(rgb, a), position: t }
}

/// A linear gradient through `stops` (positions 0..1 along the angle), each alpha times `a`.
fn linear(angle: f32, stops: &[(f32, [u8; 3], f32)], a: f32) -> Brush {
    Brush::LinearGradient(LinearGradientBrush::new(angle, stops.iter().map(|&(t, c, al)| stop(t, c, al * a))))
}

/// Whether a look's shapes sit on whole pixels and are drawn without anti-aliasing.
pub fn crisp(look: &Look) -> bool {
    look.style == Style::Bars
}

/// The brushes of a frame's paths.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brushes {
    pub fill: Brush,
    pub caps: Brush,
    pub line: Brush,
    pub glow: Brush,
    pub refl: [Brush; STRIPS],
    pub line_width: f32,
    pub glow_width: f32,
    /// The ring's bloom behind the cover.
    pub bloom: Brush,
    /// The ring's ripples, and their line's width.
    pub ripples: Brush,
    pub ripple_width: f32,
    /// The ring's snare rings, and their line's width.
    pub rings: Brush,
    pub ring_width: f32,
    /// Sparks: the young ones' dots, the old ones', the discs of light round them.
    pub sparks: Brush,
    pub sparks_old: Brush,
    pub sparks_glow: Brush,
}

/// The brushes for a look on a stage: `a` is its strength where it shows (`Look::strength`:
/// behind the page, or in the panel and on the full page).
pub fn brushes(look: &Look, stage: &Stage, top_h: f32, refl_h: f32, stops: &[(f32, [u8; 3], f32)], a: f32) -> Brushes {
    let across = look.colour_by == ColourBy::Frequency && !matches!(look.colours, Colours::Accent);
    // Mirrored layouts: the gradient across runs out from the middle both ways.
    let mirrored_stops = |s: &[(f32, [u8; 3], f32)]| -> Vec<(f32, [u8; 3], f32)> {
        let mut v: Vec<_> = s.iter().rev().map(|&(t, c, al)| (0.5 - t / 2.0, c, al)).collect();
        v.extend(s.iter().map(|&(t, c, al)| (0.5 + t / 2.0, c, al)));
        v
    };
    let mut b = Brushes { line_width: 2.0, glow_width: 0.0, ..Brushes::default() };
    // A spark's dot: the look's colours, a third of the way to white (an ember is hot).
    let hot: Vec<(f32, [u8; 3], f32)> = stops.iter().map(|&(t, c, al)| (t, c.map(|v| (f32::from(v) + (255.0 - f32::from(v)) * 0.35).round() as u8), al.max(0.75))).collect();
    match look.style {
        Style::Radial | Style::Saturn | Style::Galaxy | Style::Swarm | Style::Eclipse => {
            let g = halo(stage);
            let (cx, cy) = (g.cx, g.cy);
            // The farthest the drawing goes: where the ripples are gone.
            let reach = g.hub + g.gap + g.travel + 1.0;
            let foot = (g.hub + g.gap) / reach;
            let tip = (g.hub + g.gap + g.len) / reach;
            // Out from the cover: the gradient's foot at the contour's, its top at `top` (a
            // share of the way to the reach), each colour's strength what `alpha` says there.
            let radial_of = |stops: &[(f32, [u8; 3], f32)], top: f32, alpha: &dyn Fn(f32) -> f32| {
                let samples = 8;
                Brush::RadialGradient(
                    RadialGradientBrush::new_circle(std::iter::once(stop(0.0, stops[0].1, alpha(0.0) * a)).chain((0..samples).map(|j| {
                        let t = j as f32 / (samples - 1) as f32;
                        let (c, al) = sample(stops, t);
                        stop(foot + t * (top - foot), c, al.max(0.75) * alpha(t) * a)
                    })))
                    .with_center(cx, cy)
                    .with_radius(reach),
                )
            };
            let radial = |top: f32, alpha: &dyn Fn(f32) -> f32| radial_of(stops, top, alpha);
            if across {
                // Around the cover: low at the top, high at the bottom, both sides alike.
                let mut v: Vec<_> = stops.iter().map(|&(t, c, al)| (t / 2.0, c, al.max(0.8))).collect();
                v.extend(stops.iter().rev().map(|&(t, c, al)| (1.0 - t / 2.0, c, al.max(0.8))));
                let conic = |alpha: f32| Brush::ConicGradient(ConicGradientBrush::new(0.0, v.iter().map(|&(t, c, al)| stop(t, c, al * alpha * a))).with_center(cx, cy));
                b.line = conic(1.0);
                b.glow = conic(0.36);
            } else {
                b.line = radial(tip, &|_| 1.0);
                b.glow = radial(tip, &|_| 0.36);
            }
            // The corona: light at the cover's edge, thinning toward the contour (one brush
            // cannot both turn with the ring and fade outward: it fades).
            b.fill = radial(tip, &|t| 0.5 * (1.0 - t).powf(1.5) + 0.04);
            b.caps = b.fill.clone();
            // The ripples: through the colours on their way out, gone at the reach (a ripple
            // cannot fade by its age in one path: where it is says how old it is).
            b.ripples = radial(1.0, &|t| 0.95 * (1.0 - t.powf(1.5)).powf(0.9));
            b.ripple_width = (g.len * 0.022).clamp(1.25, 2.25);
            // The snare's rings: the look's colours most of the way to white (they are to be
            // told from the ripples at a glance), bright for most of their way and gone
            // where it ends.
            let glare: Vec<(f32, [u8; 3], f32)> = stops.iter().map(|&(t, c, al)| (t, c.map(|v| (f32::from(v) + (255.0 - f32::from(v)) * 0.7).round() as u8), al.max(0.75))).collect();
            b.rings = radial_of(&glare, (g.hub + g.gap + RING_REACH * g.travel) / reach, &|t| 1.0 - t * t * t);
            b.ring_width = b.ripple_width;
            // The sparks: as the ripples, fainter the farther out (and smaller the older).
            b.sparks = radial_of(&hot, 1.0, &|t| 0.95 * (1.0 - t).powf(0.6));
            b.sparks_old = radial_of(&hot, 1.0, &|t| 0.5 * (1.0 - t).powf(0.6));
            b.sparks_glow = radial(1.0, &|t| 0.2 * (1.0 - t).powf(0.6));
            b.line_width = (g.len * 0.03).clamp(1.5, 3.0);
            b.glow_width = g.glow;
            // The bloom: light from behind the cover, strongest at its edge, gone well before
            // the reach; the page makes it brighter with the low notes (`Paths::pulse`).
            let (c, _) = sample(stops, 0.3);
            let at = g.hub / reach;
            b.bloom = Brush::RadialGradient(
                RadialGradientBrush::new_circle([stop(0.0, c, 0.0), stop((at - 0.02).max(0.0), c, 0.0), stop(at, c, 0.5 * a), stop(at + (1.0 - at) * 0.2, c, 0.2 * a), stop(at + (1.0 - at) * 0.6, c, 0.0), stop(1.0, c, 0.0)])
                    .with_center(cx, cy)
                    .with_radius(reach),
            );
            if look.style == Style::Saturn {
                // The dust: the colours round the ring (by frequency), or out from the cover;
                // the far side's grains fainter than the near side's. Its rim is a thin line.
                if across {
                    let mut v: Vec<_> = hot.iter().map(|&(t, c, al)| (t / 2.0, c, al)).collect();
                    v.extend(hot.iter().rev().map(|&(t, c, al)| (1.0 - t / 2.0, c, al)));
                    let conic = |alpha: f32| Brush::ConicGradient(ConicGradientBrush::new(0.0, v.iter().map(|&(t, c, al)| stop(t, c, al * alpha * a))).with_center(cx, cy));
                    b.sparks = conic(0.95);
                    b.sparks_old = conic(0.5);
                    b.sparks_glow = conic(0.16);
                } else {
                    b.sparks = radial_of(&hot, 1.0, &|_| 0.95);
                    b.sparks_old = radial_of(&hot, 1.0, &|_| 0.5);
                    b.sparks_glow = radial(1.0, &|_| 0.16);
                }
                b.line_width = 1.5;
                b.glow_width = (g.glow * 0.5).max(3.0);
            }
            if matches!(look.style, Style::Galaxy | Style::Swarm) {
                // Dust lit by its notes, not thrown: as bright far out as by the cover, the
                // colours running outward.
                b.sparks = radial_of(&hot, 1.0, &|_| 0.95);
                b.sparks_old = radial_of(&hot, 1.0, &|_| 0.45);
                b.sparks_glow = radial(1.0, &|_| 0.16);
            }
            if look.style != Style::Radial {
                // The other kinds have a thin contour (or a rim), not the Ripples' band.
                b.line_width = 1.5;
                b.glow_width = (g.glow * 0.5).max(3.0);
            }
        }
        Style::Bonfire => {
            // Across the page (low notes' colour on the left), or up it: a spark then turns
            // through the colours as it rises.
            let grad = |s: &[(f32, [u8; 3], f32)], alpha: f32| linear(if across { 90.0 } else { 0.0 }, s, a * alpha);
            b.fill = grad(stops, 0.3);
            b.line = grad(stops, 1.0);
            b.caps = b.fill.clone();
            b.sparks = grad(&hot, 0.95);
            b.sparks_old = grad(&hot, 0.5);
            b.sparks_glow = grad(stops, 0.2);
            b.line_width = 2.0;
        }
        _ => {
            let fill_alpha = if look.style == Style::Line { 0.3 } else { 1.0 };
            let up = |alpha: f32| -> Brush {
                if across {
                    let s = if look.mirror { mirrored_stops(stops) } else { stops.to_vec() };
                    linear(90.0, &s, a * alpha)
                } else {
                    // Up the bars: the gradient over the bars' room (the box less the caps').
                    let room = (top_h - 4.0).max(1.0) / top_h.max(1.0);
                    let s: Vec<_> = stops.iter().map(|&(t, c, al)| (t * room, c, al)).collect();
                    linear(0.0, &s, a * alpha)
                }
            };
            b.fill = up(fill_alpha);
            if look.style == Style::Line {
                // The line's area: faint at the foot, a little stronger under the line, so
                // the shape reads and the bottom does not become a band.
                b.fill = if across {
                    let s = if look.mirror { mirrored_stops(stops) } else { stops.to_vec() };
                    linear(90.0, &s, a * 0.16)
                } else {
                    let room = (top_h - 4.0).max(1.0) / top_h.max(1.0);
                    let s: Vec<_> = stops.iter().map(|&(t, c, al)| (t * room, c, al * (0.04 + 0.36 * t))).collect();
                    linear(0.0, &s, a)
                };
            }
            b.caps = up(1.0);
            if look.style == Style::Line {
                // No glow under the line: a second wide stroke of
                // a few hundred curves cost more than it showed.
                b.line = up(1.0);
                b.line_width = 2.0;
            }
            if refl_h > 0.0 && look.style == Style::Line {
                // The mirrored area, faint near the baseline and stronger toward its edge
                // (the line's mirror image), fading out below. A mirrored stroke of the
                // line read better still, but cost a third more to draw (measured).
                let st = look.reflection_strength;
                if across {
                    let s = if look.mirror { mirrored_stops(stops) } else { stops.to_vec() };
                    for (k, r) in b.refl.iter_mut().enumerate() {
                        let mid = (k as f32 + 0.5) / STRIPS as f32;
                        *r = linear(90.0, &s, a * st * (0.3 + 0.6 * mid) * fade(mid));
                    }
                } else {
                    let samples = 9;
                    b.refl[0] = Brush::LinearGradient(LinearGradientBrush::new(
                        180.0,
                        (0..samples).map(|j| {
                            let t = j as f32 / (samples - 1) as f32;
                            let (c, al) = sample(stops, t);
                            stop(t, c, al * (0.1 + 1.2 * t) * fade(t) * a * st)
                        }),
                    ));
                }
            } else if refl_h > 0.0 {
                let strength = look.reflection_strength * fill_alpha.max(0.6);
                if across {
                    // Strips, fainter down: the fade the brush cannot hold.
                    let s = if look.mirror { mirrored_stops(stops) } else { stops.to_vec() };
                    for (k, r) in b.refl.iter_mut().enumerate() {
                        let mid = (k as f32 + 0.5) / STRIPS as f32;
                        *r = linear(90.0, &s, a * strength * fade(mid));
                    }
                } else {
                    // The mirror of the bars' gradient, top down, fading as it goes.
                    let samples = 9;
                    let v: Vec<_> = (0..samples)
                        .map(|j| {
                            let t = j as f32 / (samples - 1) as f32;
                            let (c, al) = sample(stops, t);
                            (t, c, al * fade(t))
                        })
                        .collect();
                    b.refl[0] = Brush::LinearGradient(LinearGradientBrush::new(180.0, v.iter().map(|&(t, c, al)| stop(t, c, al * a * strength))));
                }
            }
        }
    }
    b
}

/// The reflection's fade at depth `t` (0 at the baseline, 1 at its foot).
pub fn fade(t: f32) -> f32 {
    (1.0 - t.clamp(0.0, 1.0)).powf(1.6)
}

/// A preview of a colour scheme for its swatch (left to right).
pub fn swatch(look: &Look, colours: Colours, accent: [u8; 3], palette: &[[u8; 3]]) -> Brush {
    let l = Look { colours, ..look.clone() };
    let mut s = base_stops(&l, accent, palette);
    if colours == Colours::Accent {
        s.iter_mut().for_each(|x| x.2 = 0.6 + 0.4 * x.0);
    }
    linear(90.0, &s, 1.0)
}

#[cfg(test)]
#[path = "trace.rs"]
mod trace;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visualizer::look::{Preset, ThemeViz};

    fn stage(w: f32, h: f32) -> Stage {
        Stage::new(w, h, (10.0, 4.0))
    }

    fn look(style: Style) -> Look {
        Look { style, reflection: true, reflection_size: 0.5, peaks: true, ..Look::default() }
    }

    /// The points of a path (M, L, C, Q, H, V, Z and arcs' end points), in order.
    fn points(path: &str) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let mut cmd = 'M';
        let mut nums: Vec<f32> = Vec::new();
        let flush = |cmd: char, nums: &mut Vec<f32>, x: &mut f32, y: &mut f32, out: &mut Vec<(f32, f32)>| {
            match cmd {
                'H' => nums.iter().for_each(|&v| {
                    *x = v;
                    out.push((*x, *y));
                }),
                'V' => nums.iter().for_each(|&v| {
                    *y = v;
                    out.push((*x, *y));
                }),
                'A' => nums.chunks(7).filter(|c| c.len() == 7).for_each(|c| {
                    (*x, *y) = (c[5], c[6]);
                    out.push((*x, *y));
                }),
                _ => nums.chunks(2).filter(|c| c.len() == 2).for_each(|c| {
                    (*x, *y) = (c[0], c[1]);
                    out.push((*x, *y));
                }),
            }
            nums.clear();
        };
        let mut num = String::new();
        for ch in path.chars().chain(std::iter::once(' ')) {
            if ch.is_ascii_alphabetic() || ch == ' ' {
                if let Ok(v) = num.parse() {
                    nums.push(v);
                }
                num.clear();
                if ch.is_ascii_alphabetic() {
                    flush(cmd, &mut nums, &mut x, &mut y, &mut out);
                    cmd = ch;
                }
            } else {
                num.push(ch);
            }
        }
        flush(cmd, &mut nums, &mut x, &mut y, &mut out);
        out
    }

    #[test]
    fn silence_draws_nothing() {
        for style in Style::ALL {
            let mut d = Draw::default();
            let l = look(style);
            let s = stage(800.0, 300.0);
            let (_, bands) = layout(&l, &s);
            let p = d.draw(&l, &s, true, &vec![0.0; bands], &vec![0.0; bands]).clone();
            assert!(p.fill.is_empty() && p.caps.is_empty() && p.refl.iter().all(String::is_empty), "{style:?}: {p:?}");
            assert!(p.sparks.is_empty() && p.sparks_old.is_empty() && p.sparks_glow.is_empty() && p.front.is_empty() && p.front_glow.is_empty(), "{style:?}: no sparks from silence");
            if style == Style::Radial {
                assert!(!p.line.is_empty() && p.ripples.is_empty(), "the ring's circle stays, and nothing leaves it");
            } else {
                assert!(p.line.is_empty(), "{style:?}");
            }
        }
    }

    #[test]
    fn a_bar_stands_where_its_band_is_and_as_high_as_its_level() {
        let l = Look { reflection: false, ..look(Style::Bars) };
        let s = stage(280.0, 104.0);
        let (n, bands) = layout(&l, &s);
        assert_eq!((n, bands), (20, 20), "14 px a bar");
        let mut levels = vec![0.0; bands];
        levels[3] = 0.5;
        let mut d = Draw::default();
        let p = d.draw(&l, &s, true, &levels, &[0.0; 20]).clone();
        let pts = points(&p.fill);
        let xs: Vec<f32> = pts.iter().map(|p| p.0).collect();
        let ys: Vec<f32> = pts.iter().map(|p| p.1).collect();
        let (minx, maxx) = (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(0.0, f32::max));
        // Twenty bars of 10 with gaps of 4 take 276 px: centred, 2 px in.
        assert!((minx - (2.0 + 3.0 * 14.0)).abs() < 0.11 && (maxx - (2.0 + 3.0 * 14.0 + 10.0)).abs() < 0.11, "the fourth bar: {minx}..{maxx}");
        let top = ys.iter().copied().fold(f32::MAX, f32::min);
        assert!((top - (104.0 - 0.5 * 100.0)).abs() < 0.11, "half of the room: {top}");
    }

    #[test]
    fn loud_levels_bend_instead_of_pinning() {
        assert_eq!(shape(0.5), 0.5);
        assert!(shape(1.0) < 0.96 && shape(1.0) > shape(0.9) && shape(1.2) < 0.999 && shape(3.0) <= 1.0);
        assert!((shape(0.8001) - 0.8001).abs() < 1e-3, "continuous at the knee");
    }

    #[test]
    fn the_reflection_mirrors_the_bars_below_the_baseline_and_fades() {
        let l = look(Style::Bars);
        let s = stage(140.0, 150.0);
        let (top, refl) = split(&l, s.h, true);
        assert_eq!((top, refl), (100.0, 50.0));
        let (_, bands) = layout(&l, &s);
        let mut levels = vec![0.0; bands];
        levels[2] = 0.6;
        let mut d = Draw::default();
        let p = d.draw(&l, &s, true, &levels, &vec![0.0; bands]).clone();
        let r = points(&p.refl[0]);
        let depth = r.iter().map(|p| p.1).fold(0.0, f32::max);
        assert!((depth - 0.6 * 50.0).abs() < 0.11, "a scaled mirror: {depth}");
        assert!(fade(0.0) == 1.0 && fade(0.5) < 0.4 && fade(1.0) == 0.0);
        // A gradient across: in strips, each clipped to its own band of the reflection.
        let la = Look { colours: Colours::Rainbow, colour_by: ColourBy::Frequency, ..l.clone() };
        let p = d.draw(&la, &s, true, &levels, &vec![0.0; bands]).clone();
        assert!(!p.refl[0].is_empty() && p.refl[STRIPS - 1].is_empty(), "60% deep: the last strip is empty");
        for k in 0..STRIPS {
            let (s0, s1) = strip_bounds(50.0, k);
            assert!(points(&p.refl[k]).iter().all(|q| q.1 >= s0 - 0.06 && q.1 <= s1 + 0.06), "strip {k}");
        }
        // Not where the stage has no room for it (the ring), or off.
        assert_eq!(split(&look(Style::Radial), 150.0, true), (150.0, 0.0));
        assert_eq!(split(&Look { reflection: false, ..l.clone() }, 150.0, true), (150.0, 0.0));
        assert_eq!(split(&l, 150.0, false), (150.0, 0.0));
    }

    #[test]
    fn mirrored_bars_put_the_low_notes_in_the_middle() {
        let l = Look { mirror: true, reflection: false, ..look(Style::Bars) };
        let s = stage(280.0, 104.0);
        let (n, bands) = layout(&l, &s);
        assert_eq!((n, bands), (20, 10));
        let order: Vec<usize> = order(bands, true).collect();
        assert_eq!(order[9], 0);
        assert_eq!(order[10], 0);
        assert_eq!(order[0], 9);
        assert_eq!(order[19], 9);
    }

    #[test]
    fn the_line_follows_the_levels_and_spans_the_width() {
        let l = Look { reflection: false, peaks: false, ..look(Style::Line) };
        let s = stage(400.0, 103.0);
        let (n, bands) = layout(&l, &s);
        let levels: Vec<f32> = (0..bands).map(|i| if i == bands / 2 { 0.8 } else { 0.2 }).collect();
        let mut d = Draw::default();
        let p = d.draw(&l, &s, true, &levels, &vec![0.0; bands]).clone();
        let pts = points(&p.line);
        assert_eq!(pts.first().unwrap().0, 0.0);
        assert!((pts.last().unwrap().0 - 400.0).abs() < 0.11, "{n} points to the right edge");
        let top = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        assert!(top < 103.0 - 0.45 * 100.0, "the peak shows (smoothed): {top}");
        assert!(pts.iter().all(|p| p.1 <= 103.0 + 0.01), "never under the baseline");
    }

    /// The closed curves of a path: for each, the ends of its segments (the first again last).
    fn loops(path: &str) -> Vec<Vec<(f32, f32)>> {
        path.split('M')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let pts = points(&format!("M{s}"));
                std::iter::once(pts[0]).chain(pts[1..].chunks(3).filter(|c| c.len() == 3).map(|c| c[2])).collect()
            })
            .collect()
    }

    #[test]
    fn the_ring_surrounds_its_hub_the_low_notes_swelling_it_the_rest_shaping_it() {
        let l = look(Style::Radial);
        for s in [stage(600.0, 400.0), Stage { corner: 12.0, ..stage(600.0, 400.0) }, Stage { hub: 120.0, ..stage(360.0, 360.0) }] {
            let g = halo(&s);
            let (n, bands) = layout(&l, &s);
            assert_eq!(n, bands * 2, "mirrored");
            assert!(g.len < g.travel && g.hub + g.gap + g.len <= s.w.min(s.h) / 2.0 && g.travel >= g.hub, "the contour inside the stage, the ripples a cover's radius out: {g:?}");
            let foot = g.hub + g.gap;
            let dist = |q: &(f32, f32)| ((q.0 - g.cx).powi(2) + (q.1 - g.cy).powi(2)).sqrt();
            // The low notes alone: the whole ring swells, evenly.
            let mut levels = vec![0.0; bands];
            levels[..bands / 8].iter_mut().for_each(|v| *v = 1.0);
            let mut d = Draw::default();
            let p = d.draw(&l, &s, true, &levels, &levels).clone();
            let edge = points(&p.line);
            assert!(edge.iter().all(|q| q.0 >= 0.0 && q.0 <= s.w && q.1 >= 0.0 && q.1 <= s.h), "inside the stage: {s:?}");
            assert!(edge.iter().all(|q| dist(q) >= foot - 0.2), "outside the cover");
            if g.corner.is_none() {
                let r: Vec<f32> = edge.iter().map(dist).collect();
                let (lo, hi) = (r.iter().copied().fold(f32::MAX, f32::min), r.iter().copied().fold(0.0, f32::max));
                assert!(hi - lo < 1.5 && lo > foot + g.len * 0.15, "an even swell: {lo}..{hi}");
            }
            assert!(p.pulse > 0.9, "and the bloom with it");
            // The corona: the contour, and the cover's place left out of it (a second loop,
            // on the foot, the other way round).
            let fill = loops(&p.fill);
            assert!(p.fill.matches('M').count() == 2 && p.fill.matches('Z').count() == 2, "{}", p.fill);
            assert!(fill[0].iter().all(|q| dist(q) >= foot - 0.2), "the contour outside the cover");
            if g.corner.is_none() {
                assert!(p.fill.contains('A') && points(&p.fill).iter().rev().take(2).all(|q| (dist(q) - foot).abs() < 0.2), "the foot's circle: {}", p.fill);
            }
            // The low mids: the top; the highs: the bottom.
            let mut levels = vec![0.0; bands];
            levels[bands / 8 + 1] = 1.0;
            let p = d.draw(&l, &s, true, &levels, &levels).clone();
            let edge = points(&p.line);
            let top = edge.iter().copied().fold((0.0, f32::MAX), |a, q| if q.1 < a.1 { q } else { a });
            assert!((top.0 - g.cx).abs() < 30.0 && top.1 < g.cy - foot - g.len * 0.1, "{top:?} {g:?}");
            let bottom = edge.iter().copied().fold((0.0, 0.0f32), |a, q| if q.1 > a.1 { q } else { a });
            assert!((bottom.1 - (g.cy + foot)).abs() < 1.0, "{bottom:?}");
        }
        // The page's own cover: the hub is as big as the page says.
        assert_eq!(halo(&Stage { hub: 120.0, ..stage(360.0, 360.0) }).hub, 120.0);
        assert!((halo(&stage(400.0, 400.0)).hub - (200.0 - EDGE) * HUB).abs() < 1e-3);
    }

    #[test]
    fn the_ring_has_no_seam_and_its_halves_mirror() {
        let l = look(Style::Radial);
        let s = stage(500.0, 500.0);
        let (n, bands) = layout(&l, &s);
        // Every band its own level: the highest meet at the bottom, the lowest at the top.
        let levels: Vec<f32> = (0..bands).map(|i| 0.2 + 0.6 * (i as f32 / bands as f32)).collect();
        let mut d = Draw::default();
        let p = d.draw(&l, &s, true, &levels, &vec![0.0; bands]).clone();
        let on = loops(&p.line).remove(0);
        assert_eq!(on.len(), n + 1, "closed: back to the first");
        let g = halo(&s);
        for k in 0..n / 2 {
            let (a, b) = (on[k], on[n - 1 - k]);
            assert!((a.1 - b.1).abs() < 0.15 && ((a.0 - g.cx) + (b.0 - g.cx)).abs() < 0.15, "mirrored: {k} {a:?} {b:?}");
        }
        // Where the halves meet, neighbours are as close as anywhere else: no step.
        let r = |q: (f32, f32)| ((q.0 - g.cx).powi(2) + (q.1 - g.cy).powi(2)).sqrt();
        let steps: Vec<f32> = (0..n).map(|k| (r(on[k]) - r(on[(k + 1) % n])).abs()).collect();
        let at_bottom = steps[n / 2 - 1];
        let at_top = steps[n - 1];
        let typical = steps.iter().copied().fold(0.0, f32::max);
        assert!(at_bottom <= typical && at_top <= typical && at_bottom < 0.5 && at_top < 0.5, "{at_bottom} {at_top} {typical}");
    }

    #[test]
    fn a_beat_sends_the_contour_out_as_a_ripple_that_flattens_and_goes() {
        let l = look(Style::Radial);
        let s = stage(600.0, 600.0);
        let g = halo(&s);
        let (_, bands) = layout(&l, &s);
        let foot = g.hub + g.gap;
        let r = |q: &(f32, f32)| ((q.0 - g.cx).powi(2) + (q.1 - g.cy).powi(2)).sqrt();
        let spread = |c: &[(f32, f32)]| c.iter().map(r).fold(0.0, f32::max) - c.iter().map(r).fold(f32::MAX, f32::min);
        // A quiet moment with a shape to it, then a beat (the low notes up, and down again).
        let quiet: Vec<f32> = (0..bands).map(|i| if i < bands / 8 { 0.1 } else { 0.15 + 0.5 * ((i as f32) * 0.21).sin().abs() }).collect();
        let loud: Vec<f32> = quiet.iter().enumerate().map(|(i, &v)| if i < bands / 8 { 0.9 } else { v }).collect();
        let mut d = Draw::default();
        let dt = 1.0 / 60.0;
        let frame = |d: &mut Draw, levels: &[f32]| {
            d.tick(dt);
            d.draw(&l, &s, true, levels, levels).ripples.clone()
        };
        // Nothing leaves at once: the first comes with the beat, or after a rest.
        for _ in 0..15 {
            assert!(frame(&mut d, &quiet).is_empty(), "no ripple yet");
        }
        frame(&mut d, &loud);
        let born = frame(&mut d, &quiet);
        assert_eq!(loops(&born).len(), 1, "the beat topped out: one ripple");
        // On its way out: farther each frame, flatter, never past its room.
        let mut last = loops(&born).remove(0);
        let first_spread = spread(&last);
        for _ in 0..80 {
            let now = loops(&frame(&mut d, &quiet)).remove(0);
            let (a, b) = (last.iter().map(r).sum::<f32>() / last.len() as f32, now.iter().map(r).sum::<f32>() / now.len() as f32);
            assert!(b > a, "farther out: {a} then {b}");
            assert!(now.iter().all(|q| r(q) <= foot + g.travel + 0.2 && r(q) >= foot - 0.2), "between the cover and the room's edge");
            last = now;
        }
        assert!(spread(&last) < first_spread * 0.6, "flatter: {} then {}", first_spread, spread(&last));
        // With sound and nothing happening in it, one after a long lull (not by a clock that
        // can be heard as one); never more than the most at once.
        let (mut most, mut left) = (0, 0);
        for _ in 0..600 {
            most = most.max(loops(&frame(&mut d, &quiet)).len());
            left += d.ripples.list.iter().filter(|r| r.age == 0.0).count();
        }
        assert!((1..=ripple_room(&g).0).contains(&most) && (2..=(10.0 / RIPPLE_LULL) as usize).contains(&left), "{most} at once, {left} left");
        // Silence: those on their way finish it, and no more leave.
        let silent = vec![0.0; bands];
        for _ in 0..(RIPPLE_LIFE / dt) as usize + 2 {
            frame(&mut d, &silent);
        }
        assert!(frame(&mut d, &silent).is_empty(), "all gone");
        // A still frame (no time passed): a few, spread over the room, the same each time.
        let mut d = Draw::default();
        let a = d.draw(&l, &s, true, &quiet, &quiet).ripples.clone();
        let b = d.draw(&l, &s, true, &quiet, &quiet).ripples.clone();
        assert!(a == b && loops(&a).len() == ripple_room(&g).0.min(RIPPLE_STILL.len()), "{}", loops(&a).len());
        // Less room, fewer of them: behind the page (the cover takes most of its stage) and
        // in the panel, they are not crowded.
        // A ripple is gone sooner there too: the same speed, a shorter way.
        let (small, big) = (ripple_room(&halo(&stage(300.0, 300.0))), ripple_room(&halo(&stage(1100.0, 1100.0))));
        assert!(small.0 < big.0 && small.1 < big.1 && big.0 == RIPPLE_MAX && big.1 == RIPPLE_LIFE, "{small:?} {big:?}");
        // Intensity: the same music, a bigger swing.
        let swing = |intensity: f32| {
            let l = Look { intensity, ..look(Style::Radial) };
            let mut d = Draw::default();
            points(&d.draw(&l, &s, true, &quiet, &quiet).line).iter().map(r).fold(0.0, f32::max) - foot
        };
        assert!(swing(0.5) < swing(1.0) && swing(1.0) < swing(2.0) && swing(2.0) <= g.len + 0.5, "{} {} {}", swing(0.5), swing(1.0), swing(2.0));
        // Another style, and back: none carried over.
        d.tick(dt);
        d.draw(&look(Style::Bars), &s, true, &quiet, &quiet);
        d.tick(dt);
        assert!(d.draw(&l, &s, true, &quiet, &quiet).ripples.is_empty());
    }

    #[test]
    fn every_kick_has_its_ripple_and_the_field_moves_with_the_beat() {
        let l = look(Style::Radial);
        let s = stage(1100.0, 1100.0);
        let g = halo(&s);
        let (_, bands) = layout(&l, &s);
        let (most, _) = ripple_room(&g);
        assert_eq!(most, RIPPLE_MAX);
        let foot = g.hub + g.gap;
        let r = |q: &(f32, f32)| ((q.0 - g.cx).powi(2) + (q.1 - g.cy).powi(2)).sqrt();
        let quiet: Vec<f32> = (0..bands).map(|i| if i < bands / 8 { 0.1 } else { 0.15 + 0.5 * ((i as f32) * 0.21).sin().abs() }).collect();
        let loud: Vec<f32> = quiet.iter().enumerate().map(|(i, &v)| if i < bands / 8 { 0.9 } else { v }).collect();
        let dt = 1.0 / 60.0;
        let frame = |d: &mut Draw, levels: &[f32], hit: f32, snap: f32, period: f32| {
            d.tick(dt);
            d.pulse(Pulse { hit, snap, period, bright: 0.6, ..Pulse::default() });
            d.draw(&l, &s, true, levels, levels).clone()
        };
        // A kick on every beat at 120 a minute, the tempo heard a hair long and a hair short
        // (a way's time of just over and just under five beats: with room for five, the
        // sixth kick used to find the first one's ripple still on its way and go without).
        for heard in [0.5015, 0.4985] {
            let mut d = Draw::default();
            let (mut kicks, mut left) = (0, 0);
            for i in 0..30 * 40 {
                let kick = i % 30 == 0;
                frame(&mut d, &quiet, if kick { 0.7 } else { 0.0 }, 0.0, if i < 90 { 0.0 } else { heard });
                let new = d.ripples.list.iter().filter(|r| r.age == 0.0).count();
                kicks += usize::from(kick);
                left += new;
                assert_eq!(new, usize::from(kick), "frame {i}: a ripple on the kick, none between");
                assert!(d.ripples.list.iter().filter(|r| !r.leaving).count() <= most && d.ripples.list.len() <= most + 1, "frame {i}: {} on their way", d.ripples.list.len());
            }
            assert_eq!((kicks, left), (40, 40));
            // In time: those on their way about a beat's share of the way apart.
            let far: Vec<f32> = d.ripples.list.iter().filter(|r| !r.leaving).map(|r| r.far).collect();
            assert!(far.len() >= most - 1 && far.windows(2).all(|w| w[0] > w[1] + 0.08), "{far:?}");
        }
        // A kick pushes those on their way: they surge with it, and drift between kicks.
        let mut d = Draw::default();
        for i in 0..30 * 8 - 1 {
            frame(&mut d, &quiet, if i % 30 == 0 { 0.7 } else { 0.0 }, 0.0, 0.5);
        }
        let before = d.ripples.list[1].far;
        frame(&mut d, &quiet, 0.0, 0.0, 0.5);
        let drifted = d.ripples.list[1].far - before;
        frame(&mut d, &quiet, 0.7, 0.0, 0.5);
        let pushed = d.ripples.list[1].far - before - drifted;
        assert!(drifted > 0.0 && pushed > drifted * 2.5, "a frame's way: {drifted} between kicks, {pushed} on one");
        // A harder kick throws its ripple faster; and the low notes' swell in its first
        // moments says how hard it was (the kick is told before its peak).
        let flown = |hit: f32, levels: &[f32]| {
            let mut d = Draw::default();
            frame(&mut d, &quiet, hit, 0.0, 0.5);
            for _ in 0..12 {
                frame(&mut d, levels, 0.0, 0.0, 0.5);
            }
            d.ripples.list[0].far
        };
        let (soft, hard, loudest) = (flown(0.3, &quiet), flown(1.0, &quiet), flown(0.3, &loud));
        assert!(soft < hard && hard < loudest && soft > 0.05, "{soft} {hard} {loudest}");
        // A snare sends a ring: one closed curve off the contour, shown two frames on (by
        // then a kick it might have been the edge of has been told), quickly out to the end
        // of the ripples' way; and no ripple.
        let mut d = Draw::default();
        let p = frame(&mut d, &quiet, 0.0, 0.8, 0.5);
        assert!(d.ripples.rings.len() == 1 && p.rings.is_empty() && p.ripples.is_empty());
        assert!(frame(&mut d, &quiet, 0.0, 0.0, 0.5).rings.is_empty(), "not before it is known for a snare's");
        let mut last = foot;
        for i in 0..56 {
            let p = frame(&mut d, &quiet, 0.0, 0.0, 0.5);
            let ring = loops(&p.rings).remove(0);
            let mean = ring.iter().map(r).sum::<f32>() / ring.len() as f32;
            assert!(mean > last && ring.iter().all(|q| r(q) <= foot + g.travel + 0.2), "frame {i}: {mean} after {last}");
            last = mean;
        }
        assert!((last - foot - RING_REACH * g.travel).abs() < g.travel * 0.06, "at its way's end: {last}");
        for _ in 0..6 {
            frame(&mut d, &quiet, 0.0, 0.0, 0.5);
        }
        assert!(frame(&mut d, &quiet, 0.0, 0.0, 0.5).rings.is_empty(), "gone after two beats");
        // A roll: never more than there is room for, the oldest making room.
        for i in 0..60 {
            frame(&mut d, &quiet, 0.0, if i % 7 == 0 { 0.6 } else { 0.0 }, 0.5);
            assert!(d.ripples.rings.iter().filter(|r| r.shown && !r.leaving).count() <= RING_MAX && d.ripples.rings.len() <= RING_MAX + 2);
        }
        assert!(d.ripples.rings.len() >= 3);
        // A snap is any sharp edge. A kick's own (told a frame before the kick, the sound
        // bright for that frame only) and a plucked note's (never bright) send no ring; a
        // clap on a kick does (still bright two frames on).
        let pulse = |d: &mut Draw, hit: f32, snap: f32, bright: f32| {
            d.tick(dt);
            d.pulse(Pulse { hit, snap, period: 0.5, bright, ..Pulse::default() });
            d.draw(&l, &s, true, &quiet, &quiet).clone()
        };
        let mut d = Draw::default();
        for i in 0..30 * 8 {
            let p = pulse(&mut d, if i % 30 == 1 { 0.9 } else { 0.0 }, if i % 30 == 0 { 0.9 } else { 0.0 }, if i % 30 == 0 { 0.4 } else { 0.004 });
            assert!(p.rings.is_empty(), "a kick's edge, frame {i}");
        }
        let mut d = Draw::default();
        for i in 0..30 * 8 {
            let p = pulse(&mut d, 0.0, if i % 15 == 0 { 0.8 } else { 0.0 }, 0.03);
            assert!(p.rings.is_empty(), "a plucked note, frame {i}");
        }
        let mut d = Draw::default();
        let mut shown = 0;
        for i in 0..30 * 8 {
            let clap = i % 60 < 12;
            let before = d.ripples.rings.iter().filter(|r| r.shown).count();
            pulse(&mut d, if i % 30 == 1 { 0.9 } else { 0.0 }, if i % 30 == 0 { 0.9 } else { 0.0 }, if i % 30 == 0 { 0.4 } else if clap { 0.14 } else { 0.004 });
            shown += usize::from(d.ripples.rings.iter().filter(|r| r.shown).count() > before);
        }
        assert_eq!(shown, 4, "one for each clap");
        // With a beat to go by, what sends a ripple depends on where in the beat it is: on
        // the beat any kick told, or an onset of the low notes; on the half beat a firm kick
        // the low notes bear out; off both nothing (a bass line's notes fall anywhere).
        let sent = |hit: f32, low: f32, phase: f32| {
            let mut d = Draw::default();
            d.tick(dt);
            d.pulse(Pulse { hit, period: 0.5, phase, low, ..Pulse::default() });
            !d.draw(&l, &s, true, &quiet, &quiet).ripples.is_empty()
        };
        // (A kick is told a twentieth of a beat after it started, at this tempo.)
        for (hit, low, phase, goes) in [
            (0.35, 0.0, 0.05, true),
            (0.35, 0.0, 0.97, true),
            (0.0, 4.0, 0.06, true),
            (0.0, 4.0, 0.95, false),
            (0.0, 2.0, 0.06, false),
            (0.6, 9.0, 0.55, false),
            (0.8, 2.0, 0.55, false),
            (0.8, 4.0, 0.55, true),
            (0.0, 9.0, 0.55, false),
            (1.0, 9.0, 0.3, false),
            (1.0, 9.0, 0.8, false),
        ] {
            assert_eq!(sent(hit, low, phase), goes, "a hit of {hit}, an onset of {low}, at {phase} of the beat");
        }
        // While no tempo is known a weak hit is no kick (music without any has them all the
        // time): no ripple, and the music still counts as having no beat.
        let mut d = Draw::default();
        for i in 0..120 {
            d.tick(dt);
            d.pulse(Pulse { hit: if i % 12 == 0 { 0.44 } else { 0.0 }, snap: 0.0, period: 0.0, ..Pulse::default() });
            assert!(d.draw(&l, &s, true, &quiet, &quiet).ripples.is_empty(), "frame {i}");
        }
        d.tick(dt);
        d.pulse(Pulse { hit: 0.8, snap: 0.0, period: 0.0, ..Pulse::default() });
        assert_eq!(loops(&d.draw(&l, &s, true, &quiet, &quiet).ripples).len(), 1, "a real one");
        // Without kicks (a breakdown, the tempo still held): nothing leaves by the clock;
        // a ripple leaves when the music swells and the swell tops out.
        let mut d = Draw::default();
        for i in 0..120 {
            assert!(frame(&mut d, &quiet, 0.0, 0.0, 0.5).ripples.is_empty(), "frame {i}: nothing to send one");
        }
        assert!(frame(&mut d, &loud, 0.0, 0.0, 0.5).ripples.is_empty(), "still rising");
        assert!(frame(&mut d, &quiet, 0.0, 0.0, 0.5).ripples.is_empty(), "the swell topped out: its ripple waits for the beat");
        frame(&mut d, &quiet, 0.0, 0.0, 0.5);
        d.tick(dt);
        d.pulse(Pulse { period: 0.5, beat: true, ..Pulse::default() });
        assert_eq!(loops(&d.draw(&l, &s, true, &quiet, &quiet).ripples).len(), 1, "and leaves on it");
        // With kicks, a swell sends none of its own.
        let mut d = Draw::default();
        let mut left = 0;
        for i in 0..30 * 6 {
            frame(&mut d, if i % 30 < 5 { &loud } else { &quiet }, if i % 30 == 0 { 0.7 } else { 0.0 }, 0.0, 0.5);
            left += d.ripples.list.iter().filter(|r| r.age == 0.0).count();
        }
        assert_eq!(left, 6);
    }

    /// The centres of a path of dots (each two arcs: its leftmost point, its rightmost, back).
    fn dots(path: &str) -> Vec<(f32, f32)> {
        points(path).chunks(3).filter(|c| c.len() == 3).map(|c| ((c[0].0 + c[1].0) / 2.0, c[0].1)).collect()
    }

    #[test]
    fn sparks_leave_where_it_is_loud_go_out_and_go_out() {
        let dt = 1.0 / 60.0;
        // The ring's: off the contour, outward, a burst on a beat, never more than the most.
        let l = look(Style::Radial);
        let s = stage(700.0, 700.0);
        let g = halo(&s);
        let (_, bands) = layout(&l, &s);
        let r = |q: &(f32, f32)| ((q.0 - g.cx).powi(2) + (q.1 - g.cy).powi(2)).sqrt();
        let quiet: Vec<f32> = (0..bands).map(|i| if i < bands / 8 { 0.1 } else { 0.15 + 0.5 * ((i as f32) * 0.21).sin().abs() }).collect();
        let loud: Vec<f32> = quiet.iter().enumerate().map(|(i, &v)| if i < bands / 8 { 0.9 } else { v }).collect();
        let mut d = Draw::default();
        let count = |p: &Paths| dots(&p.sparks).len() + dots(&p.sparks_old).len();
        let frame = |d: &mut Draw, look: &Look, st: &Stage, levels: &[f32]| {
            d.tick(dt);
            d.draw(look, st, true, levels, levels).clone()
        };
        let mut before = 0;
        for _ in 0..30 {
            before = count(&frame(&mut d, &l, &s, &quiet));
        }
        assert!(before > 0, "a trickle while there is sound");
        frame(&mut d, &l, &s, &loud);
        let after = frame(&mut d, &l, &s, &quiet);
        assert!(count(&after) >= before + 10, "a beat's burst: {before} then {}", count(&after));
        assert!(dots(&after.sparks).iter().all(|q| r(q) >= g.hub - 1.0), "outside the cover");
        assert!(dots(&after.sparks_glow).len() <= count(&after) && !after.sparks_glow.is_empty(), "discs of light round them");
        let mut most = 0;
        for _ in 0..400 {
            most = most.max(count(&frame(&mut d, &l, &s, &loud)).max(count(&frame(&mut d, &l, &s, &quiet))));
        }
        assert!(most <= 240, "{most}");
        // Silence: those on their way finish it.
        let silent = vec![0.0; bands];
        for _ in 0..200 {
            frame(&mut d, &l, &s, &silent);
        }
        assert_eq!(count(&frame(&mut d, &l, &s, &silent)), 0);
        // A still frame: some, the same each time.
        let mut d = Draw::default();
        let a = d.draw(&l, &s, true, &quiet, &quiet).clone();
        let b = d.draw(&l, &s, true, &quiet, &quiet).clone();
        assert!(count(&a) > 10 && a.sparks == b.sparks && a.sparks_old == b.sparks_old);
        // The bonfire's: from the foot, upward, more where it is loud; its coals a line along
        // the foot, a sixth of the height at most.
        let l = look(Style::Bonfire);
        let s = stage(900.0, 500.0);
        let (n, bands) = layout(&l, &s);
        assert_eq!(n, bands, "across the page, not mirrored");
        let left: Vec<f32> = (0..bands).map(|i| if i < bands / 3 { 0.8 } else { 0.0 }).collect();
        let mut d = Draw::default();
        let mut p = Paths::default();
        for _ in 0..90 {
            p = frame(&mut d, &l, &s, &left);
        }
        let all: Vec<(f32, f32)> = dots(&p.sparks).into_iter().chain(dots(&p.sparks_old)).collect();
        assert!(all.len() > 20, "{}", all.len());
        assert!(all.iter().filter(|q| q.0 < 450.0).count() > all.len() * 2 / 3, "most rise from the loud third");
        assert!(all.iter().all(|q| q.1 < 500.0) && all.iter().any(|q| q.1 < 380.0), "upward");
        let coals = points(&p.line);
        assert!(coals.iter().all(|q| q.1 >= 500.0 - 2.0 - 80.0 - 0.5 && q.1 <= 500.0), "low along the foot");
        assert!(p.fill.ends_with('Z') && !p.fill.is_empty());
        // Another style's sparks are not carried over.
        let ring = frame(&mut d, &look(Style::Radial), &stage(700.0, 700.0), &quiet);
        assert!(count(&ring) < 20, "{}", count(&ring));
    }

    #[test]
    fn saturn_has_its_dust_behind_the_cover_and_in_front() {
        let l = look(Style::Saturn);
        let s = stage(700.0, 700.0);
        let g = halo(&s);
        let (_, bands) = layout(&l, &s);
        let levels: Vec<f32> = (0..bands).map(|i| 0.3 + 0.5 * ((i as f32) * 0.3).sin().abs()).collect();
        let mut d = Draw::default();
        let a = d.draw(&l, &s, true, &levels, &levels).clone();
        let (back, front) = (dots(&a.sparks_old), dots(&a.front));
        assert!(back.len() > 40 && front.len() > 40 && back.len() + front.len() == ((g.hub * 1.1) as usize).clamp(90, 300), "{} behind, {} in front", back.len(), front.len());
        // Outside the cover's own circle along the ring's long axis; the ring lies flat (its
        // grains nearer the centre than they are far out along the long axis).
        let r = |q: &(f32, f32)| ((q.0 - g.cx).powi(2) + (q.1 - g.cy).powi(2)).sqrt();
        let far = back.iter().chain(&front).map(r).fold(0.0, f32::max);
        let near = back.iter().chain(&front).map(r).fold(f32::MAX, f32::min);
        assert!(far > g.hub * 1.6 && far < g.hub * 2.2 && near < g.hub * 0.7, "{near}..{far} for a cover of {}", g.hub);
        assert!(!a.line.is_empty() && !a.front_glow.is_empty() && a.fill.is_empty());
        // It turns: a frame later the grains have moved, and a beat pushes the ring out.
        d.tick(0.1);
        d.pulse(Pulse { hit: 1.0, snap: 0.0, period: 0.0, ..Pulse::default() });
        let b = d.draw(&l, &s, true, &levels, &levels).clone();
        assert!(b.front != a.front && b.pulse >= 0.99);
        let pushed = dots(&b.sparks_old).iter().chain(&dots(&b.front)).map(r).fold(0.0, f32::max);
        assert!(pushed > far, "{pushed} after a beat, {far} before");
    }

    #[test]
    fn the_other_kinds_of_ring_draw_round_the_cover_and_move_with_the_beat() {
        let s = stage(800.0, 800.0);
        let g = halo(&s);
        let r = |q: &(f32, f32)| ((q.0 - g.cx).powi(2) + (q.1 - g.cy).powi(2)).sqrt();
        let all = |p: &Paths| -> Vec<(f32, f32)> { dots(&p.sparks).into_iter().chain(dots(&p.sparks_old)).collect() };
        for style in [Style::Galaxy, Style::Swarm, Style::Eclipse] {
            let l = look(style);
            let (_, bands) = layout(&l, &s);
            let sound: Vec<f32> = (0..bands).map(|i| 0.3 + 0.5 * ((i as f32) * 0.3).sin().abs()).collect();
            let frame = |d: &mut Draw, hit: f32, levels: &[f32]| {
                d.tick(1.0 / 60.0);
                d.pulse(Pulse { hit, snap: 0.0, period: 0.5, ..Pulse::default() });
                d.draw(&l, &s, true, levels, levels).clone()
            };
            // A still frame has something to show, the same each time, round the cover.
            let mut d = Draw::default();
            let a = d.draw(&l, &s, true, &sound, &sound).clone();
            let b = d.draw(&l, &s, true, &sound, &sound).clone();
            assert!(all(&a).len() > 30 && a.sparks == b.sparks && !a.line.is_empty() && a.fill.is_empty() && a.front.is_empty(), "{style:?}: {}", all(&a).len());
            assert!(all(&a).iter().all(|q| r(q) > g.hub * 0.95), "{style:?}: outside the cover");
            // A beat: the cover's pulse and the bloom jump with it, and die away.
            let mut d = Draw::default();
            let quiet = frame(&mut d, 0.0, &sound);
            let hit = frame(&mut d, 1.0, &sound);
            assert!(quiet.punch == 0.0 && hit.punch >= 0.99 && hit.pulse >= 0.99, "{style:?}");
            let mut later = hit.clone();
            for _ in 0..40 {
                later = frame(&mut d, 0.0, &sound);
            }
            assert!(later.punch < 0.2, "{style:?}: {}", later.punch);
            // What the beat does to it.
            match style {
                // A flare from one place, out and back: gone again within its life.
                Style::Eclipse => {
                    let n1 = all(&hit).len();
                    assert!(all(&quiet).is_empty() && n1 > 10, "{n1}");
                    let far = all(&later).iter().map(r).fold(0.0, f32::max);
                    assert!(far > g.hub * 1.15 && far < g.hub * 2.6, "{far} for a cover of {}", g.hub);
                }
                // The dust is there all along; the beat blows the swarm out, and hurries the galaxy.
                Style::Swarm => {
                    let mean = |p: &Paths| all(p).iter().map(r).sum::<f32>() / all(p).len().max(1) as f32;
                    let mut blown = hit.clone();
                    for _ in 0..12 {
                        blown = frame(&mut d, 0.0, &sound);
                    }
                    let _ = blown;
                    let mut d2 = Draw::default();
                    let calm = frame(&mut d2, 0.0, &sound);
                    let mut out = frame(&mut d2, 1.0, &sound);
                    for _ in 0..10 {
                        out = frame(&mut d2, 0.0, &sound);
                    }
                    assert!(mean(&out) > mean(&calm) + g.hub * 0.05, "{} then {}", mean(&calm), mean(&out));
                    assert!(all(&out).iter().all(|q| r(q) < g.hub * 3.2), "and it stays near");
                }
                _ => {
                    assert_eq!(all(&quiet).len(), all(&later).len(), "the same stars");
                    assert!(quiet.sparks != later.sparks || quiet.sparks_old != later.sparks_old, "turning");
                }
            }
            // Silence: nothing (what was thrown finishes its way first).
            let silent = vec![0.0; bands];
            let mut p = Paths::default();
            for _ in 0..240 {
                d.tick(1.0 / 60.0);
                d.pulse(Pulse::default());
                p = d.draw(&l, &s, true, &silent, &silent).clone();
            }
            assert!(all(&p).is_empty() && p.line.is_empty(), "{style:?}");
        }
    }

    #[test]
    fn a_square_ring_follows_the_cover_round_its_corners() {
        let s = Stage { hub: 100.0, corner: 12.0, ..stage(400.0, 400.0) };
        let g = halo(&s);
        let f = g.frame(96);
        let foot = g.hub + g.gap;
        // From the top middle, clockwise, every point on the foot's rounded square, its way
        // out a unit vector square to it.
        assert!((f[0].0.0 - g.cx).abs() < 8.0 && (f[0].0.1 - (g.cy - foot)).abs() < 0.01, "{:?}", f[0]);
        assert!(f[1].0.0 > f[0].0.0, "clockwise");
        for ((x, y), (dx, dy)) in &f {
            assert!(((dx * dx + dy * dy).sqrt() - 1.0).abs() < 1e-4);
            let (u, v) = ((x - g.cx).abs(), (y - g.cy).abs());
            let q = 12.0 + g.gap;
            let s0 = foot - q;
            let on = if u <= s0 + 0.01 { (v - foot).abs() < 0.05 } else if v <= s0 + 0.01 { (u - foot).abs() < 0.05 } else { (((u - s0).powi(2) + (v - s0).powi(2)).sqrt() - q).abs() < 0.05 };
            assert!(on, "({x}, {y}) on the foot");
        }
        // The corners get points of their own, spread with the rest (not a jump of 90°).
        let turns: Vec<f32> = (0..f.len()).map(|k| { let (a, b) = (f[k].1, f[(k + 1) % f.len()].1); (a.0 * b.1 - a.1 * b.0).asin().abs() }).collect();
        assert!(turns.iter().all(|t| *t < 0.5), "{turns:?}");
        assert!(g.perimeter() < std::f32::consts::TAU * (foot + g.len * 0.5) * 1.3);
        // It draws, inside the stage: the contour, its corona and the still frame's ripples.
        let l = look(Style::Radial);
        let (_, bands) = layout(&l, &s);
        let mut d = Draw::default();
        let p = d.draw(&l, &s, true, &vec![0.6; bands], &vec![0.8; bands]).clone();
        assert!(!p.fill.is_empty() && !p.ripples.is_empty());
        for path in [&p.fill, &p.line] {
            assert!(points(path).iter().all(|q| q.0 >= 0.0 && q.0 <= 400.0 && q.1 >= 0.0 && q.1 <= 400.0));
        }
    }

    #[test]
    fn gradients_map_as_specified() {
        let l = Look { colours: Colours::Classic, ..Look::default() };
        let s = base_stops(&l, [0, 0, 0], &[]);
        assert_eq!(sample(&s, 0.0).0, [0x00, 0xc8, 0x5a], "green at the foot");
        assert_eq!(sample(&s, 1.0).0, [0xff, 0x30, 0x24], "red at the top");
        let mid = sample(&s, 0.25).0;
        assert!(mid[1] > 0xc0 && mid[0] < 0x9c, "between green and yellow-green: {mid:?}");
        // The accent: one colour, fainter at the foot.
        let a = base_stops(&Look::default(), [10, 20, 30], &[]);
        assert_eq!((sample(&a, 0.0).1, sample(&a, 1.0).1), (0.5, 1.0));
        // The cover: its palette, or three colours made from one.
        let c = Look { colours: Colours::Cover, ..Look::default() };
        assert_eq!(base_stops(&c, [0; 3], &[[1, 2, 3], [4, 5, 6]]).len(), 2);
        assert_eq!(base_stops(&c, [200, 60, 60], &[]).len(), 3);
        // Custom: the three chosen, in order.
        let u = Look { colours: Colours::Custom, custom: vec!["#ff0000".into(), "#00ff00".into(), "#0000ff".into()], ..Look::default() };
        assert_eq!(sample(&base_stops(&u, [0; 3], &[]), 0.5).0, [0, 255, 0]);
        // Up the bars, the gradient's top meets the tallest bar, not the caps' room.
        let st = stage(400.0, 104.0);
        let b = brushes(&l, &st, 104.0, 0.0, &s, 1.0);
        let Brush::LinearGradient(g) = &b.fill else { panic!("linear") };
        assert!((g.stops().last().unwrap().position - 100.0 / 104.0).abs() < 1e-4);
    }

    #[test]
    fn every_preset_draws_on_every_stage() {
        let t = ThemeViz::default();
        for preset in Preset::ALL {
            let l = preset.look(&t);
            for s in [stage(1840.0, 290.0), stage(388.0, 640.0), stage(660.0, 220.0), stage(300.0, 160.0)] {
                let (_, bands) = layout(&l, &s);
                let levels: Vec<f32> = (0..bands).map(|i| 0.3 + 0.5 * ((i as f32) * 0.3).sin().abs()).collect();
                let mut d = Draw::default();
                let p = d.draw(&l, &s, true, &levels, &levels).clone();
                assert!(!p.fill.is_empty() || !p.line.is_empty() || !p.sparks.is_empty(), "{preset:?} on {s:?}");
                let stops = base_stops(&l, [200, 120, 80], &[[40, 60, 200], [220, 80, 140], [240, 200, 90]]);
                let _ = brushes(&l, &s, p.top_h, p.refl_h, &stops, 0.8);
            }
        }
    }
}
