//! Drawing the visualizer: each style's geometry as path commands and its colours as
//! brushes, for one stage (the band behind Now playing, the panel's tab, the full page, the
//! Settings preview). No Slint state here, so each style's geometry is unit-tested;
//! `visualizer/mod.rs` hands the result to the `Visualizer` global, where a few `Path`s draw it
//! (`VisualizerStage` in vizstage.slint).
//!
//! The stage is split in two: the top box, where the bars rise from its bottom edge (the
//! baseline), and under it the reflection's box. Paths are in each box's logical pixels.
//!
//! - Bars: rounded bars, caps above them, a scaled mirror below; the low ones take the kick.
//! - Line: a smooth curve (Catmull-Rom as cubic Béziers) over a filled area; its low end
//!   takes the kick, and it thickens with the beat.
//! - Ring, around the round cover, in three kinds: Ripples (the spectrum as a glowing
//!   contour, the two halves mirrored; each beat sends the contour out as a ripple and
//!   throws sparks off it), Saturn (a tilted ring of dust in orbit, behind the cover and in
//!   front, held by the cover's weight: the inner grains outrun the outer, each beat draws it
//!   in, dust spirals down into the cover) and Eclipse (flares arcing off the rim and back).
//! - Bonfire: the spectrum as a low glowing line along the foot, sparks rising from it.
//!
//! A level is shaped before it is drawn: above 0.8 it bends toward the top instead of
//! running into it (`shape`), so loud music keeps its contour rather than a flat roof.

#[cfg(test)]
use std::fmt::Write as _;

use slint::private_unstable_api::re_exports::{ConicGradientBrush, GradientStop, LinearGradientBrush, RadialGradientBrush};
use slint::{Brush, Color};

use crate::visualizer::look::{ColourBy, Colours, Detail, Look, Motion, Style};

/// Reflection strips for a gradient across (each its own alpha; see `Paths::refl`).
pub const STRIPS: usize = 6;

/// A level as drawn: straight up to 0.8, then easing toward 1 without reaching it.
pub fn shape(x: f32) -> f32 {
    let x = x.max(0.0);
    if x <= 0.8 { x } else { 0.8 + 0.2 * ((x - 0.8) / 0.2).tanh() }
}

/// Path text, the SVG commands a `Path` is given (`commands:` in vizstage.slint): written
/// here 60 times a second and parsed back by Slint (lyon's SVG parser) before every frame is
/// drawn, so both ends are kept cheap. Each method writes one command, its letter and its
/// numbers, as `write!(out, "M{x:.1} {y:.1}")` would: the same bytes, but for a whole number
/// without its `.0` (`tenth`).
struct Svg<'a>(&'a mut String);

impl Svg<'_> {
    fn cmd(self, c: char, nums: &[f32]) -> Self {
        self.0.push(c);
        for (i, &v) in nums.iter().enumerate() {
            if i > 0 {
                self.0.push(' ');
            }
            tenth(self.0, v);
        }
        self
    }
    fn m(self, x: f32, y: f32) -> Self {
        self.cmd('M', &[x, y])
    }
    fn l(self, x: f32, y: f32) -> Self {
        self.cmd('L', &[x, y])
    }
    fn h(self, x: f32) -> Self {
        self.cmd('H', &[x])
    }
    fn v(self, y: f32) -> Self {
        self.cmd('V', &[y])
    }
    fn q(self, (cx, cy): (f32, f32), (x, y): (f32, f32)) -> Self {
        self.cmd('Q', &[cx, cy, x, y])
    }
    fn c(self, (ax, ay): (f32, f32), (bx, by): (f32, f32), (x, y): (f32, f32)) -> Self {
        self.cmd('C', &[ax, ay, bx, by, x, y])
    }
    /// Half a circle of radius `r` to (x, y), the large way round, anticlockwise.
    fn half_turn(self, r: f32, x: f32, y: f32) -> Self {
        let s = self.cmd('A', &[r, r]);
        s.0.push_str(" 0 1 0 ");
        tenth(s.0, x);
        s.0.push(' ');
        tenth(s.0, y);
        s
    }
    fn z(self) {
        self.0.push('Z');
    }
}

/// A number as `{:.1}` writes it (to a tenth, a tie to the even tenth), but without
/// `core::fmt`'s exact-decimal machinery (most of what writing the paths cost), and a whole
/// number without its `.0` (the bars' text a quarter shorter). An `f32` times ten is exact in an `f64` (24 bits and 4), so
/// the tenth rounded is the very one `{:.1}` writes. Never what the parser rejects or reads
/// otherwise: no exponent, no `-0` (written `0`), NaN as 0 and anything past a billion pixels
/// held there (`{:.1}` would write `NaN` or `inf`, and the whole path would not be drawn).
fn tenth(out: &mut String, v: f32) {
    #[cfg(test)]
    if tests::AS_BEFORE.get() {
        let _ = write!(out, "{v:.1}");
        return;
    }
    let v = if v.is_nan() { 0.0 } else { v.clamp(-1e9, 1e9) };
    let t = (f64::from(v) * 10.0).round_ties_even() as i64;
    if t < 0 {
        out.push('-');
    }
    let (mut whole, frac) = (t.unsigned_abs() / 10, (t.unsigned_abs() % 10) as u8);
    let mut buf = [0u8; 12];
    let mut i = buf.len();
    if frac != 0 {
        i -= 2;
        buf[i] = b'.';
        buf[i + 1] = b'0' + frac;
    }
    loop {
        i -= 1;
        buf[i] = b'0' + (whole % 10) as u8;
        whole /= 10;
        if whole == 0 {
            break;
        }
    }
    out.push_str(std::str::from_utf8(&buf[i..]).unwrap_or_default());
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
    /// The most room the reflection's box has at the stage's foot (behind the page: what
    /// lies under the player bar, `VisualizerStage.refl-room`); below 0: no bound.
    pub refl_room: f32,
}

impl Stage {
    pub fn new(w: f32, h: f32, bar: (f32, f32)) -> Stage {
        Stage { w, h, bar, hub: 0.0, corner: -1.0, refl_room: -1.0 }
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
    /// How much wider than its brush says the line's glow is drawn now (0: as it says; the
    /// eclipse's rim widens with the bass).
    pub glow: f32,
    /// How much thicker than its brush says the line is drawn now (0: as it says; 1: twice:
    /// the line style with the beat).
    pub swell: f32,
    /// How much brighter the area and its reflection are drawn now (0: as their brushes say;
    /// above: brighter, the bars with the beat when asked for, `bars_flash`; below: fainter,
    /// the line's area in a quiet passage). The page raises or lowers the brushes' alpha by it.
    pub light: f32,
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
        Style::Radial | Style::Saturn | Style::Eclipse => {
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

/// The top box and the reflection's, for a stage of height `h`: the reflection's box no
/// taller than `room` (below 0: no bound; 0: none), the top box then taking the rest.
pub fn split(look: &Look, h: f32, room: f32, reflect_here: bool) -> (f32, f32) {
    if reflect_here && look.reflection && look.style.reflects() && room != 0.0 {
        let mut top = (h / (1.0 + look.reflection_size)).floor();
        if room > 0.0 {
            top = top.max((h - room).floor());
        }
        (top, h - top)
    } else {
        (h, 0.0)
    }
}

/// What was heard of the beat this frame (`beat::Tracker`), for what moves with it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pulse {
    /// A kick told: its strength (to 1), 0 for none. The looks are given only what of it
    /// falls where the music's beat is (`beat_hit`).
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
    eclipse: Eclipse,
    /// The style the sparks (and the rest of what moves by itself) are of.
    sparks_of: Option<Style>,
    /// The beat as heard for the next frame (`pulse`); without it a frame listens to the
    /// levels it is given (a test, a caller with no sound to listen to).
    heard: Option<Pulse>,
    /// Since the last beat let through to the looks (seconds; `None`: none yet), so that one
    /// beat is one hit (`one_hit`).
    hit_since: Option<f32>,
    /// How loud the music is against its loudest lately (`level`).
    loud: Loud,
    /// Bars and the line: the last beat's strength, dying away in about an eighth of a second
    /// (what lifts the low notes, `KICK_FADE`), and dying away over the beat (what brightens
    /// the bars when asked for, `bars_flash`, and thickens the line, `LIT_FADE`).
    kick: f32,
    lit: f32,
    /// Whether the bars brighten on the beat; `None`: as `MP_BARS_WITH` says (`bars_flash`).
    flash: Option<bool>,
}

/// Whether the bars' area and reflection brighten on the beat. Watched on real music on
/// 2026-11-25 the flash was too jarring, and at times came with nothing much heard: it is
/// off unless `MP_BARS_WITH=flash` asks for it (kept for another try). Read once.
fn bars_flash() -> bool {
    static FLASH: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLASH.get_or_init(|| std::env::var("MP_BARS_WITH").unwrap_or_default().split(',').any(|w| w.trim().eq_ignore_ascii_case("flash")))
}

/// Bars and the line: the share of the bands, from the lowest, that take the kick; how much a
/// full kick adds to the lowest one's level (less further up, none past that share; shaped
/// like any level, so nothing pins to the top); how much it kicks every peak cap up; and how
/// fast it dies away (seconds: mostly gone in an eighth of a second).
const KICK_SPAN: f32 = 0.2;
const KICK_LIFT: f32 = 0.35;
const KICK_CAP: f32 = 0.04;
const KICK_FADE: f32 = 0.05;
/// What a beat does to the whole of them, dying away over this share of a beat (between 0.06
/// and 0.2 s; without a tempo, 0.1 s), so it has settled before the next: the bars' area and
/// reflection that much brighter at a full beat (`Paths::light`; only if asked for,
/// `bars_flash`), the line that much thicker (`Paths::swell`: 2 px to 4 px).
const LIT_FADE: f32 = 0.22;
const LIT_BARS: f32 = 0.3;
const LIT_LINE: f32 = 1.0;
/// The line's area this much fainter as quiet as the looks get (`Loud`), as its brush says
/// as loud as the loudest lately: a drop looks fuller than a breakdown.
const LINE_QUIET: f32 = 0.45;

/// What a kick adds to band `i` of `bands` (0 the lowest), at the kick's strength now.
fn lift(kick: f32, bands: usize, i: usize) -> f32 {
    if kick <= 0.0 {
        return 0.0;
    }
    let u = i as f32 / (bands as f32 * KICK_SPAN).max(1.0);
    if u >= 1.0 { 0.0 } else { KICK_LIFT * kick * (1.0 - u * u * (3.0 - 2.0 * u)) }
}

/// Band `i`'s level and peak as drawn, with what the kick adds (`lift`) and the caps kicked
/// up by `cap`: a lifted bar carries its cap with it. Without either, exactly as without the
/// kick.
fn kicked(levels: &[f32], peaks: &[f32], i: usize, up: f32, cap: f32) -> (f32, f32) {
    if up <= 0.0 && cap <= 0.0 {
        return (get(levels, i), get(peaks, i));
    }
    let l = levels.get(i).copied().unwrap_or(0.0).max(0.0) + up;
    let pk = peaks.get(i).copied().unwrap_or(0.0).max(l) + cap;
    (shape(l), shape(pk))
}

/// How loud the music is now against the loudest it has lately been, 0..1: the auto level
/// makes a quiet verse fill the display as a loud chorus does, and how far it has slid its
/// scale (`Analyzer::level_slide_db`) is the music's own level. Eased over a second and a
/// half; the loudest lately follows it up at once and forgets slowly (a third of a dB a
/// second: a chorus is remembered through the verse after it).
#[derive(Default)]
struct Loud {
    /// The slide as given for the next frame; `None`: nobody says (a test), as loud as can be.
    given: Option<f32>,
    eased: f32,
    top: f32,
    started: bool,
}

/// How many dB under the loudest lately is as quiet as the looks get.
const LOUD_SPAN: f32 = 12.0;

impl Loud {
    /// This frame's: 1 as loud as the loudest lately, 0 `LOUD_SPAN` under it or quieter. A
    /// still frame, or nothing said: 1.
    fn step(&mut self, dt: f32) -> f32 {
        let Some(db) = self.given else { return 1.0 };
        if dt <= 0.0 {
            return 1.0;
        }
        if !self.started {
            (self.started, self.eased, self.top) = (true, db, db);
        }
        self.eased += (db - self.eased) * (1.0 - (-dt / 1.5).exp());
        self.top = (self.top - 0.33 * dt).max(self.eased);
        (1.0 - (self.top - self.eased) / LOUD_SPAN).clamp(0.0, 1.0)
    }
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
        self.eclipse = Eclipse::default();
        self.sparks_of = None;
        self.heard = None;
        self.hit_since = None;
        self.loud = Loud::default();
        (self.kick, self.lit) = (0.0, 0.0);
    }

    /// How far the analysis' auto level has slid its scale (dB, `Analyzer::level_slide_db`):
    /// the next `draw` knows from it how loud the music is against its loudest lately (Saturn's
    /// ring is fuller when it is loud, the line's area brighter). Without it, as loud as can be.
    pub fn level(&mut self, slide_db: f32) {
        self.loud.given = Some(slide_db);
    }

    /// One beat, one hit: a low notes' onset on the beat stays above what counts for two or
    /// three frames, and the kick itself may be told a frame after it, so what `beat_hit`
    /// lets through can come several frames running for one beat. A hit this soon after the
    /// last one let through is dropped (with a tempo, 0.42 of a beat, between 0.16 and
    /// 0.3 s, the ripples' own least gap; without, 0.16 s).
    fn one_hit(&mut self, hit: f32, period: f32, dt: f32) -> f32 {
        // A still frame moves nothing with the beat.
        if dt <= 0.0 {
            return hit;
        }
        if let Some(t) = &mut self.hit_since {
            *t += dt;
        }
        let gap = if period > 0.0 { (0.42 * period).clamp(RIPPLE_GAP, 0.3) } else { RIPPLE_GAP };
        if hit <= 0.0 || self.hit_since.is_some_and(|t| t < gap) {
            return 0.0;
        }
        self.hit_since = Some(0.0);
        hit
    }

    /// The paths for these levels and peaks (0..1, `bands` of them).
    pub fn draw(&mut self, look: &Look, stage: &Stage, reflect_here: bool, levels: &[f32], peaks: &[f32]) -> &Paths {
        let dt = std::mem::take(&mut self.dt);
        // Every look moves with the music's beat, not with every low note told as a kick, and
        // once a beat.
        let heard = self.heard.take().map(|h| Pulse { hit: beat_hit(&h), ..h });
        let heard = heard.map(|h| Pulse { hit: self.one_hit(h.hit, h.period, dt), ..h });
        let loud = self.loud.step(dt);
        let p = &mut self.paths;
        p.fill.clear();
        p.caps.clear();
        p.line.clear();
        p.refl.iter_mut().for_each(String::clear);
        (p.pulse, p.punch, p.glow, p.swell, p.light) = (0.0, 0.0, 0.0, 0.0, 0.0);
        p.ripples.clear();
        p.rings.clear();
        p.sparks.clear();
        p.sparks_old.clear();
        p.sparks_glow.clear();
        p.front.clear();
        p.front_glow.clear();
        let (top, refl) = split(look, stage.h, stage.refl_room, reflect_here);
        p.top_h = top;
        p.refl_h = refl;
        let strips = look.colour_by == ColourBy::Frequency && !matches!(look.colours, Colours::Accent);
        if look.style != Style::Radial {
            self.ripples = Ripples::default();
        }
        // Another style's sparks are not this one's (the ring's are measured round it, the
        // bonfire's across the page).
        if self.sparks_of != Some(look.style) {
            self.sparks = Sparks::default();
            self.saturn = Saturn::default();
            self.eclipse = Eclipse::default();
            self.sparks_of = look.style.beats().then_some(look.style);
        }
        // Bars and the line take the beat too (not with Smooth motion: Calm, and the default
        // where Windows' animations are off, do not jump). A still frame moves nothing.
        let flat = matches!(look.style, Style::Bars | Style::Line) && look.motion != Motion::Smooth;
        if flat && dt > 0.0 {
            let (hit, period) = heard.map_or((0.0, 0.0), |h| (h.hit, h.period));
            let fade = if period > 0.0 { (LIT_FADE * period).clamp(0.06, 0.2) } else { 0.1 };
            self.kick = (self.kick * (-dt / KICK_FADE).exp()).max(hit);
            self.lit = (self.lit * (-dt / fade).exp()).max(hit);
            // Gone, not a trace kept forever: at rest the frame is exactly as without them.
            if self.kick < 0.005 {
                self.kick = 0.0;
            }
            if self.lit < 0.005 {
                self.lit = 0.0;
            }
        } else {
            (self.kick, self.lit) = (0.0, 0.0);
        }
        let kick = self.kick;
        match look.style {
            Style::Bars => {
                bars(p, look, stage, levels, peaks, strips, kick);
                if self.flash.unwrap_or_else(bars_flash) {
                    p.light = LIT_BARS * self.lit;
                }
            }
            Style::Line => {
                line(p, look, stage, levels, peaks, strips, kick);
                p.swell = LIT_LINE * self.lit;
                if flat && dt > 0.0 {
                    p.light = -LINE_QUIET * (1.0 - loud);
                }
            }
            Style::Radial => radial(p, &mut self.ripples, &mut self.sparks, (dt, heard), look, stage, levels),
            Style::Bonfire => bonfire(p, &mut self.sparks, (dt, heard), look, stage, levels),
            Style::Saturn => saturn(p, &mut self.saturn, (dt, heard, loud), look, stage, levels),

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
        Svg(out).m(x, a).h(x + w).v(b).h(x).z();
    }
}

/// The reflection's strips: their bounds in the reflection box.
fn strip_bounds(refl_h: f32, k: usize) -> (f32, f32) {
    (refl_h * k as f32 / STRIPS as f32, refl_h * (k + 1) as f32 / STRIPS as f32)
}

/// Bars, the low ones lifted by `kick` (the last beat's strength, dying away: `lift`), every
/// cap kicked up a little with it.
fn bars(p: &mut Paths, look: &Look, stage: &Stage, levels: &[f32], peaks: &[f32], strips: bool, kick: f32) {
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
        let (l, pk) = kicked(levels, peaks, i, lift(kick, bands, i), KICK_CAP * kick);
        let bh = (l * room).round();
        if bh >= 1.0 {
            let top = h - bh;
            if bh > r * 2.0 && r > 0.4 {
                Svg(&mut p.fill).m(x, h).v(top + r).q((x, top), (x + r, top)).h(x + bw - r).q((x + bw, top), (x + bw, top + r)).v(h).z();
            } else {
                Svg(&mut p.fill).m(x, h).v(top).h(x + bw).v(h).z();
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
                    Svg(&mut p.refl[0]).m(x, 1.0).h(x + bw).v(rh).h(x).z();
                }
            }
        }
        // A cap only where there is something to cap: 2 px tall, just above the bar.
        if look.peaks && pk * room >= 8.0 {
            let y = (h - pk * room - 3.0).max(0.0).round();
            Svg(&mut p.caps).m(x, y).h(x + bw).v(y + 2.0).h(x).z();
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
        Svg(out).c(c1, c2, p2);
    }
}

/// The line, its low end lifted by `kick` as the bars are (`lift`).
fn line(p: &mut Paths, look: &Look, stage: &Stage, levels: &[f32], peaks: &[f32], strips: bool, kick: f32) {
    let (n, bands) = layout(look, stage);
    let h = p.top_h;
    let room = (h - 3.0).max(1.0);
    let w = stage.w;
    let dx = w / (n.max(2) - 1) as f32;
    // A light smoothing across neighbours: the line reads as one shape, not a comb.
    let raw: Vec<f32> = order(bands, look.mirror).map(|i| kicked(levels, peaks, i, lift(kick, bands, i), 0.0).0).collect();
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
    Svg(&mut p.line).m(pts[0].0, pts[0].1);
    let from = p.line.len();
    curve(&mut p.line, &pts, 0.0, h);
    // The area under it is closed by the same curve: its text again, not its numbers written twice.
    Svg(&mut p.fill).m(0.0, h).l(pts[0].0, pts[0].1);
    p.fill.push_str(&p.line[from..]);
    Svg(&mut p.fill).l(w, h).z();
    if look.peaks {
        for (k, i) in order(bands, look.mirror).enumerate() {
            let pk = kicked(levels, peaks, i, lift(kick, bands, i), 0.0).1;
            if pk * room >= 8.0 && pk > lv[k] + 0.02 {
                let (x, y) = (k as f32 * dx, h - pk * room - 2.0);
                Svg(&mut p.caps).m(x - 1.5, y).h(x + 1.5).v(y + 2.0).h(x - 1.5).z();
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
                Svg(&mut p.refl[k]).m(0.0, s0);
                for (j, &l) in lv.iter().enumerate() {
                    Svg(&mut p.refl[k]).l(j as f32 * dx, (l * rh).clamp(s0, s1));
                }
                Svg(&mut p.refl[k]).l(w, s0).z();
            }
        } else {
            let rp: Vec<(f32, f32)> = lv.iter().enumerate().map(|(k, &l)| (k as f32 * dx, (l * rh).max(1.0))).collect();
            Svg(&mut p.refl[0]).m(0.0, 1.0).l(rp[0].0, rp[0].1);
            curve(&mut p.refl[0], &rp, 1.0, rh);
            Svg(&mut p.refl[0]).l(w, 1.0).z();
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
    Svg(out).m(pts[0].0, pts[0].1);
    for i in 0..n {
        let p0 = pts[(i + n - 1) % n];
        let (p1, p2) = (pts[i], pts[(i + 1) % n]);
        let p3 = pts[(i + 2) % n];
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        Svg(out).c(c1, c2, p2);
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
/// While the music has no beat to go by, a hit weaker than the first is not taken for a kick
/// (music without any has its low notes' wavering told as weak ones, a few a second), nor
/// one weaker than the second unless the low notes bear it out (`GRID_LOW.1`). On real music
/// those of 0.55 to 0.85 had nothing audible with them about half the time (those over it, a
/// quarter); the ones the low notes bear out are kept so that a new track's first kicks are
/// taken before its tempo is known, for half a hit for nothing a minute.
const RIPPLE_SURE: (f32, f32) = (0.55, 0.85);
/// With a beat to go by, what counts as one (what sends a ripple, punches the contour, bursts
/// the sparks and moves the other looks: `beat_hit`) depends on where in the beat it is. A kick is
/// told this long after it started (seconds); it is on the beat, or on the half beat,
/// within this share of a beat. On the beat: a kick told (a weak one, under `GRID_WEAK.0`,
/// only if the low notes bear it out, `GRID_WEAK.1`), or an onset of the low notes this many
/// times the usual (`GRID_LOW.0`: a kick the tracker missed under a bass line; never much
/// before the beat, `GRID_EARLY`). On the half beat: a firm kick (`GRID_HIT`) that the low
/// notes bear out (`GRID_LOW.1`). Off both: nothing. Measured over the steady excerpts of
/// a real library (3,964 beats): of the kicks told, 57% were on the beat, 19% on the half
/// beat and 24% off both, as strong off the beat as on it (a bass line's notes, which
/// fall anywhere: they are what made the ripples leave at random). The low notes had an
/// onset of 3 times the usual on three beats in four, but just as often where nothing is
/// heard: over a minute each of 63 tracks, half of them without a clear kick (2026-11-25),
/// that rule gave three hits in five, half of them with no rise in the sound to hear (the
/// low band's and the whole sound's, against the track's own moments at random: 73% of those
/// are without one); at 8 times the usual, 27%. Weak kicks with the low notes under twice the
/// usual were 47% for nothing. With these rules (and `RIPPLE_SURE`) hits for nothing went from
/// 42 a minute to 17 (36 to 15 on the music with a clear kick, 47 to 18 on the rest), the hits
/// on a reference beat from 60% to 66% (chance: 20%), and the beats with an audible rise of
/// the low notes that had a hit from 85% to 77% (with a clear kick) and 62% to 49% (the rest).
const RIPPLE_LATE: f32 = 0.025;
const GRID_NEAR: f32 = 0.14;
const GRID_LOW: (f32, f32) = (8.0, 3.0);
const GRID_WEAK: (f32, f32) = (0.55, 2.0);
/// An onset of the low notes sends its ripple no sooner than this share of a beat before
/// the beat (the low notes waver: one that only leads up to the kick is not it).
const GRID_EARLY: f32 = 0.05;
const GRID_HIT: f32 = 0.75;

/// What of a hit heard is the music's beat, for every look that moves with it (its strength
/// to 1, 0 for none). Without a tempo to go by: a hit that is surely a kick. With one: it
/// depends on where in the beat it is (`GRID_NEAR`); on the beat a low notes' onset with no
/// kick told counts as a soft one, harder the stronger.
fn beat_hit(h: &Pulse) -> f32 {
    let hit = h.hit.clamp(0.0, 1.0);
    if h.period <= 0.0 {
        let sure = hit >= RIPPLE_SURE.1 || (hit >= RIPPLE_SURE.0 && h.low >= GRID_LOW.1);
        return if sure { hit } else { 0.0 };
    }
    let (past, to_half) = beat_place(h);
    if past.abs() <= GRID_NEAR {
        if hit > 0.0 {
            if hit < GRID_WEAK.0 && h.low < GRID_WEAK.1 { 0.0 } else { hit }
        } else if h.low >= GRID_LOW.0 && past >= -GRID_EARLY {
            (0.3 + 0.08 * (h.low - GRID_LOW.0)).min(0.7)
        } else {
            0.0
        }
    } else if to_half <= GRID_NEAR && hit >= GRID_HIT && h.low >= GRID_LOW.1 {
        hit
    } else {
        0.0
    }
}

/// Where in the beat what is told now started (it is told `RIPPLE_LATE` after): how far past
/// the beat (before it: negative), and how far from the half beat, in shares of a beat. On
/// the beat while no tempo is known.
fn beat_place(h: &Pulse) -> (f32, f32) {
    if h.period <= 0.0 {
        return (0.0, 0.5);
    }
    let at = (h.phase - RIPPLE_LATE / h.period).rem_euclid(1.0);
    (if at < 0.5 { at } else { at - 1.0 }, (at - 0.5).abs())
}
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

/// The largest dot drawn as two cubics (`dot`), and a half circle's control points' height for
/// it, in radii: the one that is least out, 1.32% of the radius (outside at 45 degrees, inside
/// at the top and bottom by as much), which at this radius is 0.05 px.
const DOT_CUBIC: f32 = 3.75;
const DOT_K: f32 = 1.3158;

/// A filled circle. A small one (a spark, a grain: most of what a frame of the halo or the
/// bonfire draws) is two cubic curves, a half circle each, not two arcs: Slint's parser turns an
/// arc into curves with trigonometry and hands on four for each, so a frame of sparks read back
/// a fifth slower as arcs. A half circle as one cubic is not exact (`DOT_K`): out by at most
/// 0.05 px at `DOT_CUBIC`, less on anything smaller, which cannot be seen. A larger one (a
/// glow's disc), where it would be more, keeps its arcs.
fn dot(out: &mut String, x: f32, y: f32, r: f32) {
    #[cfg(test)]
    let arcs = r > DOT_CUBIC || tests::AS_BEFORE.get();
    #[cfg(not(test))]
    let arcs = r > DOT_CUBIC;
    if arcs {
        return Svg(out).m(x - r, y).half_turn(r, x + r, y).half_turn(r, x - r, y).z();
    }
    // From the ends as they are written (to a tenth, where the arcs' were), so that the two
    // halves are one circle's whatever the rounding: the control points as far above its
    // middle as below, and as far as its written radius asks.
    let tenth_of = |v: f32| ((f64::from(v) * 10.0).round_ties_even() / 10.0) as f32;
    let (x0, x1) = (tenth_of(x - r), tenth_of(x + r));
    let (d, k) = (x1 - x0, tenth_of((x1 - x0) / 2.0 * DOT_K));
    // From the left end over the top to the right one, then back under it, each number from
    // where the pen is (`c`, and `s`: a curve that leaves as the last one arrived): ten small
    // numbers, where the two halves in the window's own coordinates would be twelve long ones.
    Svg(out).m(x0, y).cmd('c', &[0.0, -k, d, -k, d, 0.0]).cmd('s', &[-d, k, -d, 0.0]).z();
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
    let on_beat = heard.is_some_and(|h| h.beat);
    rip.punch = if dt > 0.0 { (rip.punch * (-dt / 0.13).exp()).max(hit) } else { 0.0 };
    // The kick's jump on top, all round; then bent toward the contour's reach, never past
    // it (`shape`).
    lv.iter_mut().for_each(|l| *l = shape((*l + 0.2 * rip.punch) * intensity));
    smooth_ring(&mut lv, 1);
    (p.pulse, p.punch) = (swell.max(rip.punch), rip.punch);
    // The contour. At rest it lies on its foot: a thin lit line around the cover.
    let edge: Vec<(f32, f32)> = frame.iter().zip(&lv).map(|(&((x, y), (dx, dy)), &l)| (x + dx * l * g.len, y + dy * l * g.len)).collect();
    let from = p.line.len();
    closed_curve(&mut p.line, &edge);
    // The corona: between the cover and the contour, one path with the cover's place left
    // out of it (the contour clockwise, the foot back the other way), so nothing is painted
    // under the cover. The contour's text again, not its numbers written twice.
    if lv.iter().any(|&l| l * g.len >= 1.0) {
        p.fill.push_str(&p.line[from..]);
        match g.corner {
            None => {
                let f = g.hub + g.gap;
                Svg(&mut p.fill).m(g.cx, g.cy - f).half_turn(f, g.cx, g.cy + f).half_turn(f, g.cx, g.cy - f).z();
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
        // How far past the beat what is told now started (the hit is already the beat's,
        // `beat_hit`; a snare's ring goes by this too).
        let (past, _) = heard.map_or((0.0, 0.5), |h| beat_place(&h));
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
        Svg(&mut p.line).m(pts[0].0, pts[0].1);
        let from = p.line.len();
        curve(&mut p.line, &pts, 0.0, foot);
        Svg(&mut p.fill).m(0.0, h).l(pts[0].0, pts[0].1);
        p.fill.push_str(&p.line[from..]);
        Svg(&mut p.fill).l(w, h).z();
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

/// The beat for a look this frame: as heard (a beat's strength, `beat_hit`; a snap's), or, nobody
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

/// With a tempo, Saturn's ring keeps time: a grain halfway out (`SATURN_MID` cover's radii)
/// goes `SATURN_BEAT` of a turn in a beat, `SATURN_SURGE` of it in a surge right after the
/// beat (dying away over `SATURN_SURGE_TIME` of a beat), the rest evenly, coasting.
const SATURN_MID: f32 = 1.58;
const SATURN_BEAT: f32 = 1.0 / 16.0;
const SATURN_SURGE: f32 = 0.5;
const SATURN_SURGE_TIME: f32 = 0.25;
/// The band of light a beat sends round the ring: how wide it is (radians either side).
const SATURN_BAND: f32 = 0.3;

/// The ring's width (cover's radii), reaching near the cover so that the inner grains' pace
/// shows against the outer ones'.
const SATURN_RING: (f32, f32) = (1.1, 1.88);
/// Pull: how far in a full beat draws the ring (a share of its distance, halfway out; more
/// nearer the cover, less further out), how fast it comes in (seconds) and how long it takes to
/// ease back out (a share of the beat; without a tempo, seconds).
const SATURN_PULL: f32 = 0.16;
const SATURN_PULL_IN: f32 = 0.045;
const SATURN_PULL_OUT: f32 = 0.35;
const SATURN_PULL_ALONE: f32 = 0.18;
/// The nearest the cover anything is drawn in the ring's plane (cover's radii), but for what
/// falls into it.
const SATURN_CLOSEST: f32 = 1.04;

/// Infall: how many grains a second leave the ring's inner edge at full bass, and how many
/// more a full beat sends; how fast a falling grain gathers speed (cover's radii a second,
/// a second, at the cover's edge, less further out as the square of the distance); how long
/// a grain fed back at the outer edge takes to brighten in, and how fast it drifts back in to
/// its place (cover's radii a second).
const SATURN_FEED: f32 = 3.0;
const SATURN_FEED_BEAT: f32 = 2.5;
const SATURN_FALL: f32 = 0.55;
const SATURN_FADE_IN: f32 = 0.7;
const SATURN_DRIFT: f32 = 0.15;

/// How much of a beat's turn Saturn's ring has made `at` (0..1) of the way through the beat.
fn saturn_turned(at: f32) -> f32 {
    (1.0 - SATURN_SURGE) * at + SATURN_SURGE * (1.0 - (-at / SATURN_SURGE_TIME).exp()) / (1.0 - (-1.0 / SATURN_SURGE_TIME).exp())
}

/// How much of a beat's turn it makes in a frame `span` of a beat long that ends `at` of the
/// way through the beat (one that began in the beat before takes the end of that one too):
/// the same however many frames a beat has.
fn saturn_frame(at: f32, span: f32) -> f32 {
    let at = at.rem_euclid(1.0);
    if span >= 1.0 {
        span
    } else if at >= span {
        saturn_turned(at) - saturn_turned(at - span)
    } else {
        saturn_turned(at) + 1.0 - saturn_turned(at - span + 1.0)
    }
}

/// The share of its distance a grain `r` cover's radii out keeps, the ring drawn in by `pull`
/// (0..1 of `SATURN_PULL`): more is taken nearer the cover.
fn saturn_drawn_in(r: f32, pull: f32) -> f32 {
    1.0 - SATURN_PULL * pull * (SATURN_MID / r).min(1.4)
}

/// How far out a grain `r` out is drawn, the ring drawn in by `pull`: never into the cover.
fn saturn_pulled(r: f32, pull: f32) -> f32 {
    (r * saturn_drawn_in(r, pull)).max(r.min(SATURN_CLOSEST))
}

/// Saturn's dust between frames.
#[derive(Default)]
struct Saturn {
    grains: Vec<Grain>,
    /// The last beat's strength, dying away: without a tempo the ring hurries with it.
    push: f32,
    /// The pull toward the cover: how far in now (0..1 of `SATURN_PULL`), and what it is
    /// coming in to (the last beat's strength, easing away).
    pull: f32,
    pull_to: f32,
    beat: Beat,
    /// The band of light the last beat sent round: how long since it left, how long it
    /// takes round (a beat), and how bright it is (0: none).
    band: f32,
    band_life: f32,
    band_hit: f32,
    /// Infall: the grains owed to it (a fraction carried over), where the search for the next
    /// one to fall goes on from, and the fed-back grains' places round the ring.
    owed: f32,
    next: usize,
    seed: u32,
}

#[derive(Default)]
struct Grain {
    /// Where on its orbit (radians), how far out now and its own place (in cover's radii),
    /// how big, how fast.
    th: f32,
    r: f32,
    home: f32,
    size: f32,
    k: f32,
    /// Infall: how fast it is falling in (cover's radii a second; 0: in the ring), and how far
    /// it has brightened in after being fed back at the outer edge (1: all the way).
    fall: f32,
    shown: f32,
    /// Where it was drawn in the last frame, how big, and its layer (0 not drawn, 1 behind the
    /// cover, 2 in front, 3 in front with a disc of light): for the tests and the trace.
    drawn: (f32, f32, f32, u8),
}

/// Saturn: a tilted ring of dust in orbit round the cover, the grains on its far side
/// behind the cover (`sparks_old`, drawn under it) and those on the near side in front
/// (`front`, drawn over it). Each grain is as big as its note is loud (the spectrum round
/// the ring, as the ring style has it) and the loud ones have a disc of light. The louder the
/// music against its loudest lately (`loud`), the fuller and brighter the ring.
///
/// The cover is drawn as a heavy body the dust is bound to. Nothing flies outward: a beat is
/// the cover pulling.
/// - The ring reaches near the cover (1.1 to 1.88 of its radius) and every grain goes as far
///   round as its distance gives it (the distance to the power -1.5, Kepler's third law), so
///   the inner edge goes round 2.3 times as fast as the outer and laps it. With a tempo the
///   ring keeps time (a sixteenth of a turn a beat halfway out, surging after each beat and
///   coasting to the next: a faster song turns faster); without one it goes at its own pace,
///   hurried by the bass and the beats.
/// - A beat draws the ring in toward the cover (up to 16% of its distance at a full beat,
///   more nearer the cover, never nearer than `SATURN_CLOSEST`), most of the way within a
///   twentieth of a second, and it turns faster as it comes in (its turn this frame over the
///   square of how far in it is drawn, as a skater's spin), then eases back out over the beat.
///   The bass does not swell it.
/// - Grains leave the ring's inner edge (its inner quarter), more with the bass and a few more
///   on a beat, and spiral down in the ring's plane, faster and faster and brighter as they
///   fall (behind the cover on the far side, over its face on the near side), gone at the
///   cover's edge; each is fed back at the outer edge, brightening in, and drifts back in to
///   its place. Neither bass nor beat, nothing falls.
///
/// (Tried on 2026-11-25 and dropped: each beat throwing dust off the ring, which read as an
/// explosion; moons on tilted ellipses of their own, which read as moons rather than as the
/// cover's pull; a spoke of light lit on each beat to show the shear, which read as a bright
/// patch standing still; gaps in the ring, which did not show.)
///
/// A still frame (dt 0, the Settings preview) draws the ring whole and calm, every grain in
/// its place.
fn saturn(p: &mut Paths, st: &mut Saturn, (dt, heard, loud): (f32, Option<Pulse>, f32), look: &Look, stage: &Stage, levels: &[f32]) {
    let (n, bands) = layout(look, stage);
    let g = halo(stage);
    let tau = std::f32::consts::TAU;
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
    let (hit, _, period) = beat_of(heard, &mut st.beat, 0.7 * swell + 0.3 * energy, dt);
    st.push = if dt > 0.0 { (st.push * (-dt * 3.5).exp()).max(hit) } else { 0.0 };
    (p.pulse, p.punch) = (swell.max(st.push), st.push);
    if dt <= 0.0 {
        st.band_hit = 0.0;
    } else if hit > 0.0 {
        (st.band, st.band_life, st.band_hit) = (0.0, if period > 0.0 { period } else { 0.5 }, 0.5 + 0.5 * hit);
    } else {
        st.band += dt;
    }
    // The pull: in fast on the beat, out slowly over it.
    if dt > 0.0 {
        if hit > 0.0 {
            st.pull_to = st.pull_to.max(hit);
        }
        st.pull += (st.pull_to - st.pull) * (1.0 - (-dt / SATURN_PULL_IN).exp());
        let out = if period > 0.0 { SATURN_PULL_OUT * period } else { SATURN_PULL_ALONE };
        st.pull_to *= (-dt / out).exp();
    } else {
        (st.pull, st.pull_to) = (0.0, 0.0);
    }

    // As many grains as the cover is big, the same ones every run.
    let most = ((g.hub * 1.1) as usize).clamp(90, 300);
    let (inner, outer) = SATURN_RING;
    if st.grains.len() != most {
        let mut seed = 0x51ed_270b;
        st.grains = (0..most)
            .map(|_| {
                let th = rand(&mut seed) * tau;
                let r = inner + (outer - inner) * rand(&mut seed).powf(0.8);
                // (Each within 1.5% of its distance's speed: none at a pace of its own, which
                // would hide the inner grains outrunning the outer.)
                let (size, k) = (0.7 + 1.5 * rand(&mut seed), 1.0 + 0.03 * (rand(&mut seed) - 0.5));
                Grain { th, r, home: r, size, k, shown: 1.0, ..Grain::default() }
            })
            .collect();
    }
    // Quieter than lately: fewer grains (always the same ones, each shrinking away as the
    // ring thins past it, none blinking in and out).
    let full = 0.4 + 0.66 * loud;
    let keep = |i: usize| ((full - (i as f32 * 0.618_034).fract()) / 0.06).clamp(0.0, 1.0);
    // How far round a grain halfway out goes this frame.
    let turn = if dt <= 0.0 {
        0.0
    } else if period > 0.0 {
        tau * SATURN_BEAT * saturn_frame(heard.map_or(0.0, |h| h.phase), dt / period)
    } else {
        dt * 0.45 * (1.0 + 1.4 * swell + 1.2 * st.push) * SATURN_MID.powf(-1.5)
    };
    if dt > 0.0 {
        // Pulled in, it turns faster: as the square of how far in.
        for q in &mut st.grains {
            let spin = saturn_drawn_in(q.r, st.pull).powi(-2);
            q.th = (q.th + turn * q.k * (q.r / SATURN_MID).powf(-1.5) * spin) % tau;
        }
    }
    // Infall: grains leave the inner edge (more with the bass, a few more on a beat), fall
    // faster and faster, and are fed back at the outer edge.
    if dt > 0.0 {
        st.owed += dt * SATURN_FEED * ((swell - 0.15) / 0.6).clamp(0.0, 1.0) + SATURN_FEED_BEAT * hit;
        let edge = inner + 0.25 * (outer - inner);
        let count = st.grains.len();
        while st.owed >= 1.0 {
            st.owed -= 1.0;
            let ready = |i: usize, q: &Grain| q.fall == 0.0 && q.shown >= 1.0 && q.r < edge && keep(i) >= 1.0;
            if let Some(i) = (0..count).map(|k| (st.next + k) % count).find(|&i| ready(i, &st.grains[i])) {
                st.grains[i].fall = 0.04;
                st.next = (i + 1) % count;
            }
        }
        for q in &mut st.grains {
            if q.fall > 0.0 {
                q.fall += dt * SATURN_FALL / (q.r * q.r);
                q.r -= q.fall * dt;
                if q.r <= 1.0 {
                    // Gone into the cover: fed back at the outer edge, somewhere round it.
                    (q.fall, q.shown, q.r) = (0.0, 0.0, outer);
                    q.th = rand(&mut st.seed) * tau;
                }
            } else {
                q.shown = (q.shown + dt / SATURN_FADE_IN).min(1.0);
                q.r = q.home.max(q.r - dt * SATURN_DRIFT);
            }
        }
    }
    // Nothing to hear: the dust fades out (and silence draws nothing).
    let shown = (energy / 0.05).min(1.0);
    st.grains.iter_mut().for_each(|q| q.drawn.3 = 0);
    if shown < 0.02 {
        return;
    }
    // A thin lit line round the cover.
    let rim: Vec<(f32, f32)> = g.frame(48.max(n / 4)).iter().map(|&(q, _)| q).collect();
    closed_curve(&mut p.line, &rim);
    let (cs, sn) = (SATURN_TURN.cos(), SATURN_TURN.sin());
    // Quieter than lately: smaller, fewer of them glowing.
    let big = (g.hub * 0.0065).clamp(0.8, 1.9) * shown * (0.8 + 0.2 * loud);
    let glows = 0.45 + 0.15 * (1.0 - loud);

    // The beat's band of light: where round the ring it is (from the front, the way the
    // ring turns) and how bright, fading as it goes.
    let band = (st.band_hit > 0.0 && st.band < st.band_life).then(|| {
        let u = st.band / st.band_life;
        (std::f32::consts::FRAC_PI_2 + tau * u, st.band_hit * (1.0 - u).powf(1.5))
    });
    let level = |th: f32| {
        let a = (th / tau).rem_euclid(1.0) * n as f32;
        let (i0, t) = (a.floor() as usize % n, a - a.floor());
        (lv[i0] * (1.0 - t) + lv[(i0 + 1) % n] * t).min(1.0)
    };
    let put = |p: &mut Paths, (x, y): (f32, f32), behind: bool, r: f32, l: f32| -> (f32, f32, f32, u8) {
        if behind {
            let r = r * 0.85;
            dot(&mut p.sparks_old, x, y, r);
            (x, y, r, 1)
        } else {
            let r = r * 1.12;
            dot(&mut p.front, x, y, r);
            // The loud ones only, and close about them: a wide flat disc reads as a bubble.
            if l > glows {
                dot(&mut p.front_glow, x, y, r * 1.9);
                (x, y, r, 3)
            } else {
                (x, y, r, 2)
            }
        }
    };
    let still = dt <= 0.0;
    for (i, q) in st.grains.iter_mut().enumerate() {
        let kept = keep(i);
        if kept <= 0.0 {
            continue;
        }
        let mut l = level(q.th);
        if let Some((at, bright)) = band {
            let d = (q.th - at + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI;
            l = (l + 0.6 * bright * (-(d / SATURN_BAND).powi(2)).exp()).min(1.0);
        }
        // A still frame: every grain in its place.
        let (r, fed) = if still { (q.home, 1.0) } else { (q.r, q.shown) };
        let mut big = big * kept * fed;
        if !still && q.fall > 0.0 {
            // Brighter as it falls, shrinking away at the cover's edge.
            l = (l + 0.7 * ((q.home - r) / (q.home - 1.0)).clamp(0.0, 1.0)).min(1.0);
            big *= ((r - 1.0) / 0.06).clamp(0.0, 1.0);
        }
        if big <= 0.0 {
            continue;
        }
        let rr = g.hub * saturn_pulled(r, st.pull);
        let (ex, ey) = (q.th.cos() * rr, q.th.sin() * rr * SATURN_TILT);
        // The far half of the orbit is behind the cover.
        q.drawn = put(p, (g.cx + ex * cs - ey * sn, g.cy + ex * sn + ey * cs), q.th.sin() < 0.0, q.size * big * (0.7 + 1.0 * l), l);
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

    /// A thin contour round the cover: the spectrum, reaching out `reach` at the most,
    /// jumping with the beat (`punch`).
    fn contour(&self, out: &mut String, reach: f32, punch: f32) {
        let mut lv: Vec<f32> = self.lv.iter().map(|&l| shape(l + 0.2 * punch)).collect();
        smooth_ring(&mut lv, 1);
        let edge: Vec<(f32, f32)> = self.g.frame(self.n).iter().zip(&lv).map(|(&((x, y), (dx, dy)), &l)| (x + dx * l * reach, y + dy * l * reach)).collect();
        closed_curve(out, &edge);
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
    /// Where round the rim the next flare leaves (0..1 from the top, clockwise), and which
    /// way it leans (0 before the first: as 1).
    at: f32,
    lean: f32,
}

/// How far round the rim each flare leaves from the last (a share of a turn): they circle
/// the cover in time.
const ECLIPSE_STEP: f32 = 0.19;

/// The eclipse: the cover as an eclipsed sun. A glow at its rim (the bloom, brighter with
/// the low notes; the line's glow wider with them) and a thin contour; on each beat a flare
/// of sparks leaves the rim, a step further round than the last (`ECLIPSE_STEP`), leaning
/// one way and the next the other, and the cover pulls it back in an arc. A hard beat throws
/// a pair, the second opposite the first. Without beats: with a tempo, a small flare when the
/// music swells (a rise in the levels topping out, no more than one a beat); without one, a
/// small one after a second and a half with sound.
fn eclipse(p: &mut Paths, st: &mut Eclipse, (dt, heard): (f32, Option<Pulse>), look: &Look, stage: &Stage, levels: &[f32]) {
    let r = Round::of(look, stage, levels);
    let g = r.g;
    let level = 0.7 * r.swell + 0.3 * r.energy;
    let (hit, _, period) = beat_of(heard, &mut st.beat, level, dt);
    // A rise in the levels topping out is followed whoever listens (`beat_of` follows it only
    // while nobody does): with a tempo and no beats, it is what sends a flare.
    let swelled = heard.is_some() && dt > 0.0 && st.beat.step(level, dt);
    st.punch = if dt > 0.0 { (st.punch * (-dt / 0.13).exp()).max(hit) } else { 0.0 };
    (p.pulse, p.punch) = (r.swell.max(st.punch), st.punch);
    p.glow = 0.5 * r.swell.clamp(0.0, 1.0);
    let foot = g.hub + g.gap;
    let most = ((g.hub * 2.2) as usize).clamp(200, 600);
    let big = (g.hub * 0.011).clamp(0.9, 3.0);
    // A flare `a` round the rim (0..1 from the top), leaning `side`.
    let flare = |st: &mut Eclipse, strength: f32, a: f32, side: f32| {
        let th = a * std::f32::consts::TAU;
        let lean = side * (0.5 + 0.5 * rand(&mut st.seed));
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
    // The next flare (or pair): a step further round than the last, leaning the other way.
    let next = |st: &mut Eclipse, strength: f32, pair: bool| {
        let (a, side) = (st.at, if st.lean < 0.0 { -1.0 } else { 1.0 });
        flare(st, strength, a, side);
        if pair {
            flare(st, 0.7, a + 0.5, side);
        }
        (st.at, st.lean) = ((a + ECLIPSE_STEP).fract(), -side);
        st.quiet = 0.0;
    };
    if dt > 0.0 {
        step(st, dt);
        st.quiet += dt;
        // Without beats: with a tempo, a swell (once a beat at the most), never the clock.
        let lull = if period > 0.0 { swelled && st.quiet >= period } else { st.quiet > 1.5 };
        if r.energy > 0.03 {
            if hit > 0.0 {
                next(st, 0.8 + 0.6 * hit, hit > 0.8);
            } else if r.energy > 0.05 && lull {
                next(st, 0.6, false);
            }
        }
    } else {
        // A still frame: two flares a moment after they left.
        *st = Eclipse::default();
        if r.energy > 0.03 {
            next(st, 1.1, false);
            next(st, 0.9, false);
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

/// A line drawn as `k` strokes without anti-aliasing, one inside the other, in place of one
/// stroke with it (`Visualizer.line-passes`): the brush of each, outermost first. A pixel
/// whose centre `n` of them cover shows the brush at `n / k` of its strength, as anti-aliasing
/// would show a pixel that much covered. Strokes of one colour laid over each other add up as
/// `1 - (1 - a1)(1 - a2)...`, so the n-th's alpha is what takes `n - 1` of them from
/// `(n - 1) / k` of the brush's alpha to `n / k` of it, worked out at each stop of the brush.
pub fn feather_passes(b: &Brush, k: usize) -> Vec<Brush> {
    (1..=k)
        .map(|n| {
            map_alpha(b, |a| {
                let upto = |m: usize| a * m as f32 / k as f32;
                let below = 1.0 - upto(n - 1);
                if below <= 0.0 { 0.0 } else { 1.0 - (1.0 - upto(n)) / below }
            })
        })
        .collect()
}

/// `b` with each stop's alpha (0..1) put through `f`: the same kind of brush, its geometry kept.
fn map_alpha(b: &Brush, f: impl Fn(f32) -> f32) -> Brush {
    let map = |s: &GradientStop| GradientStop { color: s.color.with_alpha(f(f32::from(s.color.alpha()) / 255.0).clamp(0.0, 1.0)), position: s.position };
    match b {
        Brush::SolidColor(c) => Brush::SolidColor(c.with_alpha(f(f32::from(c.alpha()) / 255.0).clamp(0.0, 1.0))),
        Brush::LinearGradient(g) => Brush::LinearGradient(LinearGradientBrush::new(g.angle(), g.stops().map(map))),
        Brush::RadialGradient(g) => {
            let mut r = if g.is_circle() { RadialGradientBrush::new_circle(g.stops().map(map)) } else { RadialGradientBrush::new_ellipse(g.stops().map(map)) };
            if let Some((cx, cy)) = g.explicit_center() {
                r = r.with_center(cx, cy);
            }
            if let Some((rx, ry)) = g.explicit_radii() {
                // `with_radii` makes an ellipse of it, even of equal radii.
                r = if g.is_circle() { r.with_radius(rx) } else { r.with_radii(rx, ry) };
            }
            Brush::RadialGradient(r)
        }
        Brush::ConicGradient(g) => {
            // Its stops come already turned by its angle (`ConicGradientBrush::new`), so they are
            // laid out again at 0; its centre is the one given, or none (NaN: the box's).
            let c = ConicGradientBrush::new(0.0, g.stops().map(map));
            let (cx, cy) = g.center_or_default(f32::NAN, f32::NAN);
            Brush::ConicGradient(if cx.is_nan() { c } else { c.with_center(cx, cy) })
        }
        _ => b.clone(),
    }
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
        Style::Radial | Style::Saturn | Style::Eclipse => {
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
        let (top, refl) = split(&l, s.h, -1.0, true);
        assert_eq!((top, refl), (100.0, 50.0));
        // Behind the page the reflection's box is no taller than the room under the bar:
        // the bars keep their height, the baseline where that room begins.
        assert_eq!(split(&l, 130.0, 30.0, true), (100.0, 30.0));
        assert_eq!(split(&l, 150.0, 88.0, true), (100.0, 50.0), "room to spare: the look's share");
        let roomy = Stage { refl_room: 30.0, ..stage(140.0, 130.0) };
        let p = Draw::default().draw(&l, &roomy, true, &[0.6; 8], &[0.0; 8]).clone();
        assert_eq!((p.top_h, p.refl_h), (100.0, 30.0));
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
        assert_eq!(split(&look(Style::Radial), 150.0, -1.0, true), (150.0, 0.0));
        assert_eq!(split(&Look { reflection: false, ..l.clone() }, 150.0, -1.0, true), (150.0, 0.0));
        assert_eq!(split(&l, 150.0, -1.0, false), (150.0, 0.0));
        assert_eq!(split(&l, 150.0, 0.0, true), (150.0, 0.0), "no room: none");
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
            d.pulse(Pulse { hit, snap, period, bright: 0.6, low: if hit > 0.0 { 3.0 } else { 0.0 }, ..Pulse::default() });
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
                frame(&mut d, &quiet, if kick { 0.9 } else { 0.0 }, 0.0, if i < 90 { 0.0 } else { heard });
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
        // the beat a kick told (a weak one only if the low notes bear it out), or a strong
        // onset of the low notes; on the half beat a firm kick the low notes bear out; off
        // both nothing (a bass line's notes fall anywhere).
        let sent = |hit: f32, low: f32, phase: f32| {
            let mut d = Draw::default();
            d.tick(dt);
            d.pulse(Pulse { hit, period: 0.5, phase, low, ..Pulse::default() });
            !d.draw(&l, &s, true, &quiet, &quiet).ripples.is_empty()
        };
        // (A kick is told a twentieth of a beat after it started, at this tempo.)
        for (hit, low, phase, goes) in [
            (0.35, 2.0, 0.05, true),
            (0.35, 2.0, 0.97, true),
            (0.35, 1.0, 0.05, false),
            (0.6, 0.0, 0.05, true),
            (0.0, 9.0, 0.06, true),
            (0.0, 9.0, 0.95, false),
            (0.0, 4.0, 0.06, false),
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
        d.pulse(Pulse { hit: 0.7, snap: 0.0, period: 0.0, low: 1.0, ..Pulse::default() });
        assert!(d.draw(&l, &s, true, &quiet, &quiet).ripples.is_empty(), "a firmer one the low notes do not bear out");
        d.tick(dt);
        d.pulse(Pulse { hit: 0.7, snap: 0.0, period: 0.0, low: 3.5, ..Pulse::default() });
        assert_eq!(loops(&d.draw(&l, &s, true, &quiet, &quiet).ripples).len(), 1, "a real one");
        let mut d = Draw::default();
        d.tick(dt);
        d.pulse(Pulse { hit: 0.9, snap: 0.0, period: 0.0, ..Pulse::default() });
        assert_eq!(loops(&d.draw(&l, &s, true, &quiet, &quiet).ripples).len(), 1, "a hard one, whatever the low notes");
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

    /// The centres of a path of dots (`dot`: each from its leftmost point round by its rightmost
    /// and back, as two curves or two arcs).
    fn dots(path: &str) -> Vec<(f32, f32)> {
        path.split('M')
            .filter(|s| !s.is_empty())
            .map(|s| (s, s.split(|c: char| c == ' ' || c.is_ascii_alphabetic()).filter_map(|n| n.parse().ok()).collect::<Vec<f32>>()))
            // Curves: the left end, then (from it) the first half's control points and its end,
            // the width away. Arcs: the left end, the radius twice.
            .map(|(s, v)| (v[0] + if s.contains('c') { v[6] / 2.0 } else { v[2] }, v[1]))
            .collect()
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
        // It turns: a frame later the grains have moved, and a beat draws the ring in.
        d.tick(0.1);
        d.pulse(Pulse { hit: 1.0, snap: 0.0, period: 0.0, ..Pulse::default() });
        let b = d.draw(&l, &s, true, &levels, &levels).clone();
        assert!(b.front != a.front && b.pulse >= 0.99);
        let pulled = dots(&b.sparks_old).iter().chain(&dots(&b.front)).map(r).fold(0.0, f32::max);
        assert!(pulled < far * 0.95, "{pulled} after a beat, {far} before");
    }

    #[test]
    fn the_eclipse_draws_round_the_cover_and_flares_with_the_beat() {
        let s = stage(800.0, 800.0);
        let g = halo(&s);
        let r = |q: &(f32, f32)| ((q.0 - g.cx).powi(2) + (q.1 - g.cy).powi(2)).sqrt();
        let all = |p: &Paths| -> Vec<(f32, f32)> { dots(&p.sparks).into_iter().chain(dots(&p.sparks_old)).collect() };
        let l = look(Style::Eclipse);
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
        assert!(all(&a).len() > 30 && a.sparks == b.sparks && !a.line.is_empty() && a.fill.is_empty() && a.front.is_empty(), "{}", all(&a).len());
        assert!(all(&a).iter().all(|q| r(q) > g.hub * 0.95), "outside the cover");
        // A beat: the cover's pulse and the bloom jump with it, and die away.
        let mut d = Draw::default();
        let quiet = frame(&mut d, 0.0, &sound);
        let hit = frame(&mut d, 1.0, &sound);
        assert!(quiet.punch == 0.0 && hit.punch >= 0.99 && hit.pulse >= 0.99);
        let mut later = hit.clone();
        for _ in 0..40 {
            later = frame(&mut d, 0.0, &sound);
        }
        assert!(later.punch < 0.2, "{}", later.punch);
        // A flare from one place, out and back: gone again within its life.
        let n1 = all(&hit).len();
        assert!(all(&quiet).is_empty() && n1 > 10, "{n1}");
        let far = all(&later).iter().map(r).fold(0.0, f32::max);
        assert!(far > g.hub * 1.15 && far < g.hub * 2.6, "{far} for a cover of {}", g.hub);
        // Silence: nothing (what was thrown finishes its way first).
        let silent = vec![0.0; bands];
        let mut p = Paths::default();
        for _ in 0..240 {
            d.tick(1.0 / 60.0);
            d.pulse(Pulse::default());
            p = d.draw(&l, &s, true, &silent, &silent).clone();
        }
        assert!(all(&p).is_empty() && p.line.is_empty());
    }

    #[test]
    fn every_look_moves_with_the_music_s_beat_not_with_every_kick_told() {
        let s = stage(800.0, 800.0);
        for style in [Style::Radial, Style::Bonfire, Style::Saturn, Style::Eclipse] {
            let l = look(style);
            let (_, bands) = layout(&l, &s);
            let sound: Vec<f32> = (0..bands).map(|i| 0.3 + 0.5 * ((i as f32) * 0.3).sin().abs()).collect();
            // A second of music with nothing told, then one frame of this.
            let last = |heard: Pulse| {
                let mut d = Draw::default();
                for _ in 0..60 {
                    d.tick(1.0 / 60.0);
                    d.pulse(Pulse { period: 0.5, phase: 0.3, ..Pulse::default() });
                    d.draw(&l, &s, true, &sound, &sound);
                }
                d.tick(1.0 / 60.0);
                d.pulse(heard);
                d.draw(&l, &s, true, &sound, &sound).clone()
            };
            let none = last(Pulse { period: 0.5, phase: 0.3, ..Pulse::default() });
            // A bass line's note: as hard as a kick, but off the beat and off the half beat.
            // (Saturn's ring goes by where in the beat it is: what the same frame without it
            // draws.)
            for phase in [0.3, 0.8] {
                let none = last(Pulse { period: 0.5, phase, ..Pulse::default() });
                assert_eq!(last(Pulse { hit: 1.0, period: 0.5, phase, low: 9.0, ..Pulse::default() }), none, "{style:?}: a hit at {phase} of the beat");
            }
            // A hit that is not surely a kick while no tempo is known.
            for hit in [0.44, 0.7] {
                let unsure = last(Pulse { hit, ..Pulse::default() });
                assert_eq!(unsure, last(Pulse::default()), "{style:?}: a hit of {hit}, no tempo");
            }
            // On the beat: a weak kick the low notes do not bear out is none, one they do is.
            let quiet = last(Pulse { period: 0.5, phase: 0.06, ..Pulse::default() });
            assert_eq!(last(Pulse { hit: 0.44, period: 0.5, phase: 0.06, low: 1.0, ..Pulse::default() }), quiet, "{style:?}: a weak kick, the low notes still");
            assert!(last(Pulse { hit: 0.44, period: 0.5, phase: 0.06, low: 2.5, ..Pulse::default() }) != quiet, "{style:?}: a weak kick the low notes bear out");
            // An onset of the low notes on the beat, no kick told, of only a few times the
            // usual: a bass line wavering, nothing.
            assert_eq!(last(Pulse { period: 0.5, phase: 0.06, low: 4.0, ..Pulse::default() }), quiet, "{style:?}: a low onset of 4");
            // A kick on the beat, or a firm one on the half beat that the low notes bear out.
            for (phase, low) in [(0.05, 0.0), (0.55, 4.0)] {
                let on = last(Pulse { hit: 1.0, period: 0.5, phase, low, ..Pulse::default() });
                assert!(on != none, "{style:?}: a kick at {phase} of the beat");
                if style != Style::Bonfire {
                    assert!(none.punch == 0.0 && on.punch >= 0.99 && on.pulse >= 0.99, "{style:?}: {} at {phase} of the beat", on.punch);
                }
            }
            // An onset of the low notes on the beat with no kick told: a soft one.
            let soft = last(Pulse { period: 0.5, phase: 0.06, low: 9.0, ..Pulse::default() });
            if style != Style::Bonfire {
                assert!((soft.punch - 0.38).abs() < 0.01, "{style:?}: {}", soft.punch);
            }
        }
    }

    /// Frames of a look at `fps` for `secs` with a tempo of `period` (the phase running on,
    /// a frame ending where the beat then is), the levels, the hit and the auto level's slide
    /// of each frame as `at` says (frame, time).
    type Told<'a> = &'a mut dyn FnMut(&mut Draw, usize, f32) -> (Vec<f32>, f32, f32);

    fn run_beats(d: &mut Draw, l: &Look, s: &Stage, fps: f32, secs: f32, period: f32, at: Told) -> Vec<Paths> {
        let dt = 1.0 / fps;
        let mut out = Vec::new();
        for i in 0..(secs * fps).round() as usize {
            let t = (i + 1) as f32 * dt;
            let (levels, hit, slide) = at(d, i, t);
            d.tick(dt);
            d.level(slide);
            // A hit told has the low notes bearing it out (a weak one counts only so, `GRID_WEAK`).
            let low = if hit > 0.0 { GRID_WEAK.1 } else { 0.0 };
            d.pulse(Pulse { hit, period, phase: if period > 0.0 { (t / period).fract() } else { 0.0 }, low, ..Pulse::default() });
            out.push(d.draw(l, s, true, &levels, &levels).clone());
        }
        out
    }

    #[test]
    fn saturn_keeps_time_and_a_beat_sends_light_round_it() {
        let l = look(Style::Saturn);
        let s = stage(700.0, 700.0);
        let (_, bands) = layout(&l, &s);
        // (The low notes quiet, so that no dust falls into the cover.)
        let sound: Vec<f32> = (0..bands).map(|i| if i < bands / 8 { 0.1 } else { 0.3 + 0.2 * ((i as f32) * 0.3).sin().abs() }).collect();
        let tau = std::f32::consts::TAU;
        // How far round (radians) each grain goes in two seconds, frame by frame.
        let turned = |fps: f32, period: f32| {
            let mut d = Draw::default();
            let mut went: Vec<f32> = Vec::new();
            let mut was: Vec<f32> = Vec::new();
            run_beats(&mut d, &l, &s, fps, 2.0, period, &mut |d, _, _| {
                let now: Vec<f32> = d.saturn.grains.iter().map(|q| q.th).collect();
                if went.is_empty() {
                    went = vec![0.0; now.len()];
                } else {
                    went.iter_mut().zip(now.iter().zip(&was)).for_each(|(w, (a, b))| *w += (a - b).rem_euclid(tau));
                }
                was = now;
                (sound.clone(), 0.0, 0.0)
            });
            let now: Vec<f32> = d.saturn.grains.iter().map(|q| q.th).collect();
            went.iter_mut().zip(now.iter().zip(&was)).for_each(|(w, (a, b))| *w += (a - b).rem_euclid(tau));
            let k: Vec<f32> = d.saturn.grains.iter().map(|q| q.k * (q.r / SATURN_MID).powf(-1.5)).collect();
            (went, k)
        };
        // With a tempo: a sixteenth of a turn a beat halfway out (four beats in two seconds at
        // 120 a minute, less the first frame's, which is not counted), the inner grains
        // faster, the same at any frame rate.
        for fps in [30.0, 60.0, 144.0] {
            let (went, k) = turned(fps, 0.5);
            for (w, k) in went.iter().zip(&k) {
                let want = tau / 16.0 * (4.0 - saturn_turned(2.0 / fps)) * k;
                assert!((w - want).abs() < want * 0.03, "{fps} fps: {w} for {want}");
            }
        }
        let (fast, _) = turned(60.0, 0.4);
        let (slow, _) = turned(60.0, 0.5);
        assert!(fast.iter().zip(&slow).all(|(f, s)| f > &(s * 1.2)), "a faster song turns faster");
        // It surges after the beat and coasts to the next.
        assert!(saturn_frame(0.05, 0.05) > 3.0 * saturn_frame(0.8, 0.05));
        assert!((saturn_turned(1.0) - 1.0).abs() < 1e-5 && (saturn_frame(0.02, 0.05) - (saturn_turned(0.02) + 1.0 - saturn_turned(0.97))).abs() < 1e-6);
        // A beat sends a band of light round: grains glow where it passes, none before it
        // or once it has gone round (quiet music has none glowing; the few grains the beat
        // sends falling into the cover glow as they fall, and are not counted).
        let quiet: Vec<f32> = vec![0.15; bands];
        let mut d = Draw::default();
        let glowing = |d: &Draw| d.saturn.grains.iter().filter(|q| q.drawn.3 == 3 && q.fall == 0.0 && q.shown >= 1.0).count();
        let mut glows = Vec::new();
        run_beats(&mut d, &l, &s, 60.0, 2.0, 0.5, &mut |d, i, _| {
            if i > 0 {
                glows.push(glowing(d));
            }
            (quiet.clone(), if i == 59 { 1.0 } else { 0.0 }, 0.0)
        });
        glows.push(glowing(&d));
        assert!(glows[..59].iter().all(|&n| n == 0), "none before the beat");
        assert!(glows[59..80].iter().filter(|&&n| n > 0).count() >= 5, "the band, round the front");
        assert!(glows[95..].iter().all(|&n| n == 0), "gone within the beat");
        // Fuller when it is loud, thinner when it is quiet, the grains going one by one
        // (none coming back while it quietens).
        let mut d = Draw::default();
        let frames = run_beats(&mut d, &l, &s, 60.0, 10.0, 0.5, &mut |_, i, _| (sound.clone(), 0.0, if i < 300 { 0.0 } else { -12.0 }));
        let count = |p: &Paths| dots(&p.sparks_old).len() + dots(&p.front).len();
        let (loud, quiet) = (count(&frames[299]), count(&frames[599]));
        assert!(loud == d.saturn.grains.len() && quiet < loud * 6 / 10 && quiet > loud / 4, "{loud} loud, {quiet} quiet");
        assert!(frames[300..560].windows(2).all(|w| count(&w[1]) <= count(&w[0])), "thinning without flicker");
        // A still frame: the same each time.
        let mut d = Draw::default();
        let a = d.draw(&l, &s, true, &sound, &sound).clone();
        assert!(d.draw(&l, &s, true, &sound, &sound) == &a);
    }

    /// Saturn as behind the page: a cover 300 px across, the stage 0.3 of the page's cover
    /// round it (`halo_room` in nowplaying.slint).
    fn saturn_stage() -> Stage {
        Stage { hub: 150.0, ..stage(585.0, 585.0) }
    }


    /// A drawn grain's place in the ring's plane: its angle (radians) and distance from the
    /// cover's middle (in cover's radii), the ring's tilt and turn undone.
    fn ring_place(g: &Halo, (x, y): (f32, f32)) -> (f32, f32) {
        let (cs, sn) = (SATURN_TURN.cos(), SATURN_TURN.sin());
        let (dx, dy) = (x - g.cx, y - g.cy);
        let (ex, ey) = (dx * cs + dy * sn, -dx * sn + dy * cs);
        ((ey / SATURN_TILT).atan2(ex), ex.hypot(ey / SATURN_TILT) / g.hub)
    }

    /// The drawn grains' distances (cover's radii), in the grains' order (not drawn: none).
    fn saturn_dist(d: &Draw, g: &Halo) -> Vec<Option<f32>> {
        d.saturn.grains.iter().map(|q| (q.drawn.3 > 0).then(|| ring_place(g, (q.drawn.0, q.drawn.1)).1)).collect()
    }

    fn median(mut v: Vec<f32>) -> f32 {
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    }

    /// Music for Saturn: loud, the low notes `lows`.
    fn saturn_sound(bands: usize, lows: f32) -> Vec<f32> {
        (0..bands).map(|i| if i < bands / 8 { lows } else { 0.4 + 0.2 * ((i as f32) * 0.3).sin().abs() }).collect()
    }

    #[test]
    fn saturn_s_pull_draws_the_ring_in_on_a_beat_spinning_it_up_and_lets_it_out() {
        let l = look(Style::Saturn);
        let s = saturn_stage();
        let g = halo(&s);
        let (_, bands) = layout(&l, &s);
        // Two seconds of music at 120 a minute, its low notes swelling and falling with nothing
        // told, and a full beat at frame 60 (or none): each grain's distance and how far round
        // it went, frame by frame (none while it falls into the cover or drifts back in).
        type Seen = Vec<Vec<Option<(f32, f32)>>>;
        let run = |hit: f32| -> Seen {
            let mut d = Draw::default();
            let mut seen = Vec::new();
            let mut was: Vec<f32> = Vec::new();
            run_beats(&mut d, &l, &s, 60.0, 2.0, 0.5, &mut |d, i, t| {
                let now: Vec<f32> = d.saturn.grains.iter().map(|q| q.th).collect();
                if i > 0 {
                    let dist = saturn_dist(d, &g);
                    let placed = |k: usize| d.saturn.grains[k].fall == 0.0 && d.saturn.grains[k].r == d.saturn.grains[k].home;
                    seen.push((0..now.len()).map(|k| dist[k].filter(|_| placed(k)).map(|r| (r, (now[k] - was.get(k).copied().unwrap_or(now[k])).rem_euclid(std::f32::consts::TAU)))).collect());
                }
                was = now;
                (saturn_sound(bands, 0.3 + 0.4 * (t * 7.0).sin().abs()), if i == 59 { hit } else { 0.0 }, 0.0)
            });
            seen
        };
        let (beat, none) = (run(1.0), run(0.0));
        // The grains in their places all the way through, in both.
        let kept: Vec<usize> = (0..beat[0].len()).filter(|&k| beat.iter().chain(&none).all(|f| f[k].is_some())).collect();
        assert!(kept.len() > beat[0].len() * 3 / 4, "{} of {} kept", kept.len(), beat[0].len());
        let dist = |run: &Seen| -> Vec<f32> { run.iter().map(|f| median(kept.iter().map(|&k| f[k].unwrap().0).collect())).collect() };
        let went = |run: &Seen, a: usize, b: usize| -> f32 { run[a..b].iter().map(|f| kept.iter().map(|&k| f[k].unwrap().1).sum::<f32>()).sum() };
        let (pull, still) = (dist(&beat), dist(&none));
        // Without a beat nothing draws it in, however the low notes swell.
        let at_rest = still[10];
        assert!(still.iter().all(|&r| (r - at_rest).abs() < 0.004), "the ring moved with no beat: {still:?}");
        // On the beat (frame 59): in by 7% of its distance or more (the median grain), most of
        // it within four frames, and back out within the beat.
        let before = pull[57];
        let least = pull[58..70].iter().copied().fold(f32::MAX, f32::min);
        assert!(least < before * 0.93 && pull[63] < before * 0.95, "{before} before the beat, {least} at the most in, {} four frames on", pull[63]);
        assert!((pull[58 + 30] - before).abs() < before * 0.03, "{} a beat later, {before} before", pull[58 + 30]);
        // Drawn in, it turns faster than the same ring with no beat, and as fast before it.
        assert!((went(&beat, 20, 50) / went(&none, 20, 50) - 1.0).abs() < 0.01);
        assert!(went(&beat, 58, 66) > went(&none, 58, 66) * 1.12, "spun up {} against {}", went(&beat, 58, 66), went(&none, 58, 66));
        // Never into the cover.
        assert!(pull.iter().all(|&r| r > 1.0));
    }

    #[test]
    fn saturn_s_kepler_ring_goes_at_the_speed_its_distance_gives_it() {
        let l = look(Style::Saturn);
        let s = saturn_stage();
        let (_, bands) = layout(&l, &s);
        // (The low notes quiet and no beat: nothing drawn in, nothing falling.)
        let mut d = Draw::default();
        let mut was: Vec<f32> = Vec::new();
        let mut went: Vec<f32> = Vec::new();
        run_beats(&mut d, &l, &s, 60.0, 2.0, 0.5, &mut |d, i, _| {
            let now: Vec<f32> = d.saturn.grains.iter().map(|q| q.th).collect();
            if i > 0 {
                went.resize(now.len(), 0.0);
                went.iter_mut().zip(now.iter().zip(&was)).for_each(|(w, (a, b))| *w += (a - b).rem_euclid(std::f32::consts::TAU));
            }
            was = now;
            (saturn_sound(bands, 0.1), 0.0, 0.0)
        });
        let grains = &d.saturn.grains;
        let (inner, outer) = grains.iter().fold((f32::MAX, 0.0f32), |(a, b), q| (a.min(q.home), b.max(q.home)));
        assert!(inner < 1.13 && outer > 1.83, "{inner}..{outer}");
        // Each grain as far round as its distance to the power -1.5 says, within its 2.5%.
        let mid = went.iter().zip(grains).map(|(w, q)| w * (q.home / SATURN_MID).powf(1.5)).collect::<Vec<_>>();
        let m = median(mid.clone());
        assert!(mid.iter().all(|v| (v / m - 1.0).abs() < 0.03), "{mid:?}");
        // The inner edge laps the outer: over twice as fast.
        let pace = |r: f32| grains.iter().zip(&went).filter(|(q, _)| (q.home - r).abs() < 0.05).map(|(_, w)| *w).sum::<f32>() / grains.iter().filter(|q| (q.home - r).abs() < 0.05).count() as f32;
        assert!(pace(1.15) > 1.8 * pace(1.83), "{} inside, {} outside", pace(1.15), pace(1.83));
    }

    #[test]
    fn saturn_s_dust_falls_into_the_cover_with_the_bass_and_the_beat_and_is_fed_back() {
        let l = look(Style::Saturn);
        let s = saturn_stage();
        let g = halo(&s);
        let (_, bands) = layout(&l, &s);
        // The low notes quiet and no beat: nothing falls.
        let mut d = Draw::default();
        run_beats(&mut d, &l, &s, 60.0, 3.0, 0.5, &mut |d, _, _| {
            assert!(d.saturn.grains.iter().all(|q| q.fall == 0.0), "fell with nothing to pull it");
            (saturn_sound(bands, 0.1), 0.0, 0.0)
        });
        // Still quiet, one beat: a few fall.
        run_beats(&mut d, &l, &s, 60.0, 0.1, 0.5, &mut |_, i, _| (saturn_sound(bands, 0.1), if i == 0 { 1.0 } else { 0.0 }, 0.0));
        let falling = d.saturn.grains.iter().filter(|q| q.fall > 0.0).count();
        assert!((1..=3).contains(&falling), "{falling} after a beat");
        // Loud, a beat every beat for six seconds: each falling grain goes in, faster and
        // faster, never more than a few at once (it stays a ring), and is fed back out at the
        // edge, brightening in.
        let mut d = Draw::default();
        let (mut most, mut gone, mut fed) = (0, 0, 0);
        let mut last: Vec<(f32, f32)> = Vec::new();
        run_beats(&mut d, &l, &s, 60.0, 6.0, 0.5, &mut |d, i, _| {
            let grains = &d.saturn.grains;
            most = most.max(grains.iter().filter(|q| q.fall > 0.0).count());
            for (q, &(r, v)) in grains.iter().zip(&last) {
                if v > 0.0 && q.fall > 0.0 {
                    assert!(q.r < r && q.fall > v, "a falling grain from {r} to {}, {v} to {}", q.r, q.fall);
                }
                if v > 0.0 && q.fall == 0.0 {
                    gone += 1;
                    assert!(q.r > 1.8 && q.shown == 0.0 && r < 1.1, "fed back at {} from {r}", q.r);
                }
                if q.shown < 1.0 {
                    fed += 1;
                }
            }
            // A falling grain is drawn where it is (drawn in as the ring is), nearer the cover
            // than the ring.
            for (q, r) in grains.iter().zip(saturn_dist(d, &g)) {
                if let (true, Some(r)) = (q.fall > 0.0 && q.r > 1.08, r) {
                    let want = saturn_pulled(q.r, d.saturn.pull);
                    assert!((r - want).abs() < want * 0.01, "drawn at {r}, falling at {} (drawn in, {want})", q.r);
                }
            }
            last = grains.iter().map(|q| (q.r, q.fall)).collect();
            (saturn_sound(bands, 0.8), if i % 30 == 29 { 1.0 } else { 0.0 }, 0.0)
        });
        assert!(gone >= 15 && fed > 0, "{gone} gone into the cover");
        assert!(most as f32 <= 0.1 * d.saturn.grains.len() as f32, "{most} falling at once");
    }

    #[test]
    fn saturn_stays_on_its_stage_and_a_still_frame_is_the_whole_ring() {
        let l = look(Style::Saturn);
        let s = saturn_stage();
        let g = halo(&s);
        let (_, bands) = layout(&l, &s);
        // Loud bass and a hard beat at 170 a minute: nothing goes further out than the ring's
        // outer edge, nor off the stage.
        let mut d = Draw::default();
        let mut far = 0.0f32;
        run_beats(&mut d, &l, &s, 60.0, 6.0, 60.0 / 170.0, &mut |d, i, t| {
            if i > 0 {
                for (x, y, r, _) in d.saturn.grains.iter().map(|q| q.drawn).filter(|q| q.3 > 0) {
                    far = far.max((x - g.cx).hypot(y - g.cy) + r);
                    assert!(x - r > 0.0 && y - r > 0.0 && x + r < s.w && y + r < s.h, "a grain off the stage at {x}, {y}");
                }
            }
            let beat = (t * 170.0 / 60.0).floor() != ((t - 1.0 / 60.0) * 170.0 / 60.0).floor();
            (saturn_sound(bands, 0.95), if beat { 1.0 } else { 0.0 }, 0.0)
        });
        assert!(far <= g.hub * SATURN_RING.1 + 10.0, "{far} px out, the ring's edge at {}", g.hub * SATURN_RING.1);
        // A still frame: every grain in its place, the same each time.
        let sound = saturn_sound(bands, 0.5);
        let a = d.draw(&l, &s, true, &sound, &sound).clone();
        for (q, r) in d.saturn.grains.iter().zip(saturn_dist(&d, &g)) {
            let r = r.expect("every grain drawn");
            assert!((r - q.home).abs() < 0.01, "a grain at {r} in a still frame, its place {}", q.home);
        }
        assert!(d.draw(&l, &s, true, &sound, &sound) == &a, "a still frame not the same twice");
    }

    #[test]
    fn the_eclipse_s_flares_circle_the_cover() {
        let l = look(Style::Eclipse);
        let s = stage(800.0, 800.0);
        let g = halo(&s);
        let (_, bands) = layout(&l, &s);
        let sound: Vec<f32> = (0..bands).map(|i| 0.3 + 0.5 * ((i as f32) * 0.3).sin().abs()).collect();
        // Where round the rim (0..1 from the top, clockwise) the motes just thrown left from.
        let new = |d: &Draw| -> Vec<f32> { d.eclipse.motes.iter().filter(|q| q.age == 0.0).map(|q| ((q.x - g.cx).atan2(g.cy - q.y) / std::f32::consts::TAU).rem_euclid(1.0)).collect() };
        let round = |a: f32, b: f32| (a - b + 0.5).rem_euclid(1.0) - 0.5;
        // A beat a flare, each a step further round than the last, leaning the other way.
        let mut d = Draw::default();
        let mut flares = Vec::new();
        run_beats(&mut d, &l, &s, 60.0, 3.0, 0.5, &mut |d, i, _| {
            if i % 30 == 1 {
                flares.push((new(d), d.eclipse.lean));
            }
            (sound.clone(), if i % 30 == 0 { 0.7 } else { 0.0 }, 0.0)
        });
        assert_eq!(flares.len(), 6);
        for (k, (at, lean)) in flares.iter().enumerate() {
            let mean = at.iter().map(|&a| round(a, k as f32 * ECLIPSE_STEP)).sum::<f32>() / at.len() as f32;
            assert!(!at.is_empty() && mean.abs() < 0.03, "flare {k}: {mean} off its place");
            assert_eq!(*lean, if k % 2 == 0 { -1.0 } else { 1.0 }, "flare {k}");
        }
        // A hard beat: a pair, the second opposite the first.
        let mut d = Draw::default();
        run_beats(&mut d, &l, &s, 60.0, 0.5, 0.5, &mut |_, i, _| (sound.clone(), if i == 29 { 1.0 } else { 0.0 }, 0.0));
        let at = new(&d);
        let (near, far) = (at.iter().filter(|&&a| round(a, 0.0).abs() < 0.1).count(), at.iter().filter(|&&a| round(a, 0.5).abs() < 0.1).count());
        assert!(near > 10 && far > 10 && near + far == at.len(), "{near} and {far} of {}", at.len());
        // With a tempo and no beats: nothing by the clock, a flare when the music swells, and
        // no more than one a beat.
        let quiet: Vec<f32> = sound.iter().map(|v| v * 0.6).collect();
        let mut d = Draw::default();
        let mut left = Vec::new();
        run_beats(&mut d, &l, &s, 60.0, 6.0, 0.5, &mut |d, i, _| {
            if !new(d).is_empty() {
                left.push(i);
            }
            (if (180..184).contains(&i) || (190..194).contains(&i) || (240..244).contains(&i) { sound.clone() } else { quiet.clone() }, 0.0, 0.0)
        });
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(left[0] > 180 && left[0] < 190 && left[1] > 240 && left[1] < 250, "{left:?}");
        // Without one: a small one after a second and a half.
        let mut d = Draw::default();
        let mut left = Vec::new();
        run_beats(&mut d, &l, &s, 60.0, 2.0, 0.0, &mut |d, i, _| {
            if !new(d).is_empty() {
                left.push(i);
            }
            (quiet.clone(), 0.0, 0.0)
        });
        assert!(left.len() == 1 && left[0] > 85 && left[0] < 95, "{left:?}");
        // The rim's glow is wider with the bass.
        let mut d = Draw::default();
        let bass: Vec<f32> = (0..bands).map(|i| if i < bands / 8 { 0.9 } else { 0.3 }).collect();
        let thin: Vec<f32> = (0..bands).map(|i| if i < bands / 8 { 0.0 } else { 0.3 }).collect();
        assert!(d.draw(&l, &s, true, &bass, &bass).glow > 0.3 && d.draw(&l, &s, true, &thin, &thin).glow < 0.01);
    }

    #[test]
    fn after_another_track_the_eclipse_flares_from_its_first_strong_kicks() {
        let l = look(Style::Eclipse);
        let s = stage(800.0, 800.0);
        let g = halo(&s);
        let (_, bands) = layout(&l, &s);
        let levels: Vec<f32> = (0..bands).map(|i| 0.3 + 0.5 * ((i as f32) * 0.3).sin().abs()).collect();
        let new = |d: &Draw| d.eclipse.motes.iter().any(|q| q.age == 0.0 && ((q.x - g.cx).powi(2) + (q.y - g.cy).powi(2)).sqrt() < g.hub * 1.2);
        // While no tempo is known (the meter starts afresh with another track): a hit that is
        // surely a kick flares at once, a weak one does not.
        for (hit, flares) in [(0.9, true), (0.7, false), (0.4, false)] {
            let mut d = Draw::default();
            d.tick(1.0 / 60.0);
            d.pulse(Pulse { hit, ..Pulse::default() });
            d.draw(&l, &s, true, &levels, &levels);
            assert_eq!(new(&d), flares, "{hit}");
        }
        // Heard: ten seconds at 124 a minute, then another track at 88, the tracker started
        // afresh between them as the visualizer does. Of the new track's first kicks, those in
        // the three seconds before a tempo can be known, each flares within 80 ms.
        const RATE: u32 = 44_100;
        let make = |bpm: f32, secs: f32| -> Vec<f32> {
            let beat = 60.0 / bpm;
            (0..(secs * RATE as f32) as usize)
                .map(|i| {
                    let t = i as f32 / RATE as f32;
                    let ph = t % beat;
                    let kick = 0.8 * (-ph / 0.11).exp() * (std::f32::consts::TAU * (50.0 * ph + 3.5 * (1.0 - (-ph / 0.05).exp()))).sin();
                    kick + 0.12 * (std::f32::consts::TAU * 55.0 * t).sin() + 0.08 * (std::f32::consts::TAU * 440.0 * t).sin()
                })
                .collect()
        };
        let mut sound = make(124.0, 10.0);
        let change = sound.len();
        sound.extend(make(88.0, 3.0));
        let (mut tr, mut d) = (crate::visualizer::beat::Tracker::default(), Draw::default());
        let (dt, size) = (1.0 / 60.0, 8192usize);
        let mut flared = Vec::new();
        let mut i = 0usize;
        loop {
            let end = (i as f32 * dt * RATE as f32) as usize;
            if end >= sound.len() {
                break;
            }
            if end >= change && (end - change) < (dt * RATE as f32) as usize {
                tr.reset();
            }
            let mut buf = vec![0.0; size];
            let from = end.saturating_sub(size);
            buf[size - (end - from)..].copy_from_slice(&sound[from..end]);
            let h = tr.update(&buf, RATE, dt);
            if end >= change {
                assert_eq!(h.period, 0.0, "a tempo {} s into the new track", (end - change) as f32 / RATE as f32);
            }
            d.tick(dt);
            d.pulse(Pulse { hit: h.hit, snap: h.snap, period: h.period, bright: h.bright, beat: h.beat, phase: h.phase, low: h.low });
            d.draw(&l, &s, true, &levels, &levels);
            if end >= change && new(&d) {
                flared.push((end - change) as f32 / RATE as f32);
            }
            i += 1;
        }
        let kicks: Vec<f32> = (0..5).map(|k| k as f32 * 60.0 / 88.0).filter(|&k| k < 2.9).collect();
        let met = kicks.iter().filter(|&&k| flared.iter().any(|&f| f >= k && f < k + 0.08)).count();
        assert!(met == kicks.len(), "{met} of the kicks at {kicks:?} flared: {flared:?}");
    }

    #[test]
    fn one_beat_is_one_hit() {
        let s = stage(800.0, 800.0);
        let dt = 1.0 / 60.0;
        for style in [Style::Radial, Style::Saturn, Style::Eclipse] {
            let l = look(style);
            let (_, bands) = layout(&l, &s);
            let sound: Vec<f32> = (0..bands).map(|i| 0.3 + 0.5 * ((i as f32) * 0.3).sin().abs()).collect();
            // Half a second of music with nothing told, then four frames on the beat: what
            // they tell, and the frames after.
            let run = |told: [(f32, f32); 4]| {
                let mut d = Draw::default();
                for i in 0..30 {
                    d.tick(dt);
                    d.pulse(Pulse { period: 0.5, phase: (i as f32 * dt / 0.5 + 0.05).fract(), ..Pulse::default() });
                    d.draw(&l, &s, true, &sound, &sound);
                }
                let mut out = Vec::new();
                for i in 0..12 {
                    let (hit, low) = told.get(i).copied().unwrap_or((0.0, 0.0));
                    d.tick(dt);
                    d.pulse(Pulse { hit, period: 0.5, phase: RIPPLE_LATE / 0.5 + i as f32 * dt / 0.5, low, ..Pulse::default() });
                    out.push(d.draw(&l, &s, true, &sound, &sound).clone());
                }
                out
            };
            // A low notes' onset on the beat for three frames, then the kick itself told: one
            // hit, the onset's, as if the onset alone had been heard.
            let once = run([(0.0, 9.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)]);
            let told = run([(0.0, 9.0), (0.0, 9.5), (0.0, 9.2), (0.9, 0.0)]);
            for (i, (a, b)) in once.iter().zip(&told).enumerate() {
                assert!(a == b, "{style:?}: frame {i} of the beat");
            }
            assert!(once[0].punch > 0.3, "{style:?}: the beat is a hit");
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

    /// A hash of what was drawn (FNV-1a), to hold a frame to the very bytes it had.
    fn hash(parts: &[&str]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for p in parts {
            for b in p.bytes().chain(std::iter::once(0)) {
                h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
            }
        }
        h
    }

    /// Classic and Prism at rest (beats heard, none of them a hit) draw what they drew before
    /// bars and the line took the beat, byte for byte: the hashes are of the frames as that
    /// build drew them, but for a whole number written without its `.0` (`tenth`; the build
    /// before that, its text with the `.0`s taken out, hashes to these).
    #[test]
    fn classic_at_rest_is_drawn_as_it_was() {
        let t = ThemeViz::default();
        for (preset, want) in [(Preset::Classic, 0x6801_5b47_722b_44d3_u64), (Preset::Prism, 0x7365_a6a4_0f66_beff)] {
            let l = preset.look(&t);
            let s = stage(1840.0, 290.0);
            let (_, bands) = layout(&l, &s);
            let mut d = Draw::default();
            let mut all = Vec::new();
            for f in 0..12 {
                let levels: Vec<f32> = (0..bands).map(|i| 0.2 + 0.6 * ((i as f32) * 0.21 + f as f32 * 0.4).sin().abs()).collect();
                let peaks: Vec<f32> = levels.iter().map(|v| v + 0.07).collect();
                d.tick(1.0 / 60.0);
                d.pulse(Pulse { period: 0.5, phase: (f as f32 / 30.0).fract(), ..Pulse::default() });
                d.level(-2.0);
                let p = d.draw(&l, &s, true, &levels, &peaks).clone();
                assert!(p.light == 0.0 && p.swell == 0.0, "{preset:?}: the brushes as they are");
                all.push(hash(&[&p.fill, &p.caps, &p.line, &p.refl[0], &p.refl[1], &p.refl[2], &p.refl[3], &p.refl[4], &p.refl[5]]).to_string());
            }
            assert_eq!(hash(&all.iter().map(String::as_str).collect::<Vec<_>>()), want, "{preset:?}");
        }
    }

    /// Each bar's left edge and top, left to right (one sub-path a bar).
    fn bar_tops(path: &str) -> Vec<(f32, f32)> {
        path.split('M').filter(|s| !s.is_empty()).map(|s| points(&format!("M{s}")).iter().fold((f32::MAX, f32::MAX), |(x, y), p| (x.min(p.0), y.min(p.1)))).collect()
    }

    /// One frame of bars or the line, `dt` after the last, with a hit of `hit` (0: none).
    fn kick_frame(d: &mut Draw, l: &Look, s: &Stage, levels: &[f32], dt: f32, hit: f32) -> Paths {
        d.tick(dt);
        d.pulse(Pulse { hit, ..Pulse::default() });
        d.draw(l, s, true, levels, levels).clone()
    }

    #[test]
    fn a_kick_lifts_the_low_bars_at_once_less_further_up_and_dies_away() {
        let l = Look { reflection: false, ..look(Style::Bars) };
        let s = stage(1400.0, 204.0);
        let (n, bands) = layout(&l, &s);
        assert_eq!(n, bands);
        let levels = vec![0.3; bands];
        let mut d = Draw::default();
        let rest = bar_tops(&kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 0.0).fill);
        let p = kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 1.0);
        let hit = bar_tops(&p.fill);
        assert_eq!(rest.len(), bands);
        let span = (bands as f32 * KICK_SPAN) as usize;
        let rise: Vec<f32> = rest.iter().zip(&hit).map(|(a, b)| a.1 - b.1).collect();
        assert!(rise[0] > 40.0, "the lowest bar takes the kick in the frame it lands: {}", rise[0]);
        assert!(rise.windows(2).take(span).all(|w| w[1] <= w[0]), "less further up: {rise:?}");
        assert!(rise[span / 2] > 0.0 && rise[span / 2] < rise[0], "{rise:?}");
        assert!(rise[span..].iter().all(|&r| r == 0.0), "none past the lowest fifth: {rise:?}");
        // Shaped like any level: a full kick on a loud bar bends toward the top, not past it.
        let loud = vec![0.9; bands];
        let mut e = Draw::default();
        kick_frame(&mut e, &l, &s, &loud, 1.0 / 60.0, 0.0);
        let top = bar_tops(&kick_frame(&mut e, &l, &s, &loud, 1.0 / 60.0, 1.0).fill)[0].1;
        assert!(top > 4.0, "nothing pinned to the top: {top}");
        // Every cap is kicked up a little, the low ones riding their bars.
        let caps = |p: &Paths| bar_tops(&p.caps);
        let mut f = Draw::default();
        let c0 = caps(&kick_frame(&mut f, &l, &s, &levels, 1.0 / 60.0, 0.0));
        let c1 = caps(&kick_frame(&mut f, &l, &s, &levels, 1.0 / 60.0, 1.0));
        assert_eq!(c0.len(), bands);
        assert!(c0.iter().zip(&c1).all(|(a, b)| b.1 < a.1), "every cap jumps");
        assert!(c0[0].1 - c1[0].1 > c0[bands - 1].1 - c1[bands - 1].1 + 20.0, "the low ones with their bars");
        // The field does not brighten with it, unless asked for (`MP_BARS_WITH=flash`); then
        // it does, and has settled by the next beat.
        assert_eq!(p.light, 0.0, "no flash");
        let mut lit = Draw { flash: Some(true), ..Draw::default() };
        kick_frame(&mut lit, &l, &s, &levels, 1.0 / 60.0, 0.0);
        let flash = kick_frame(&mut lit, &l, &s, &levels, 1.0 / 60.0, 1.0).light;
        assert!((flash - LIT_BARS).abs() < 1e-4, "{flash}");
        for _ in 0..40 {
            kick_frame(&mut lit, &l, &s, &levels, 1.0 / 60.0, 0.0);
        }
        assert_eq!(kick_frame(&mut lit, &l, &s, &levels, 1.0 / 60.0, 0.0).light, 0.0, "the flash settled");
        // An eighth of a second on, the lift is nearly gone; it is gone soon after.
        let mut later = Vec::new();
        for _ in 0..8 {
            later = bar_tops(&kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 0.0).fill);
        }
        assert!(rest[0].1 - later[0].1 < 0.15 * rise[0], "{} of {}", rest[0].1 - later[0].1, rise[0]);
        for _ in 0..40 {
            kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 0.0);
        }
        let settled = kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 0.0);
        assert_eq!(bar_tops(&settled.fill), rest);
    }

    #[test]
    fn the_light_settles_before_the_next_beat() {
        let l = look(Style::Bars);
        let s = stage(1400.0, 204.0);
        let levels = vec![0.3; layout(&l, &s).1];
        for period in [0.35, 0.5, 0.8] {
            // With the flash asked for (`MP_BARS_WITH=flash`).
            let mut d = Draw { flash: Some(true), ..Draw::default() };
            let dt = 1.0 / 60.0;
            let frames = (period / dt) as usize;
            let mut lights = Vec::new();
            for i in 0..frames {
                d.tick(dt);
                d.pulse(Pulse { hit: if i == 0 { 1.0 } else { 0.0 }, period, ..Pulse::default() });
                lights.push(d.draw(&l, &s, true, &levels, &levels).light);
            }
            assert!(lights[0] > 0.29 && lights[frames - 1] < 0.02, "{period}: {lights:?}");
        }
    }

    #[test]
    fn mirrored_bars_take_the_kick_in_the_middle() {
        let l = Look { reflection: false, mirror: true, ..look(Style::Bars) };
        let s = stage(1400.0, 204.0);
        let (n, bands) = layout(&l, &s);
        assert_eq!(n, bands * 2);
        let levels = vec![0.3; bands];
        let mut d = Draw::default();
        let rest = bar_tops(&kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 0.0).fill);
        let hit = bar_tops(&kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 1.0).fill);
        let rise: Vec<f32> = rest.iter().zip(&hit).map(|(a, b)| a.1 - b.1).collect();
        assert!(rise[bands - 1] > 40.0 && rise[bands] > 40.0, "the middle: {rise:?}");
        assert!(rise[0] == 0.0 && rise[n - 1] == 0.0, "not the edges");
        let span = (bands as f32 * KICK_SPAN) as usize;
        assert!(rise[..bands - span].iter().chain(&rise[bands + span..]).all(|&r| r == 0.0));
    }

    #[test]
    fn smooth_motion_and_other_looks_do_not_take_the_kick() {
        let s = stage(1400.0, 204.0);
        for l in [Look { motion: Motion::Smooth, ..look(Style::Bars) }, Look { motion: Motion::Smooth, ..look(Style::Line) }, Preset::Calm.look(&ThemeViz::default())] {
            let levels = vec![0.3; layout(&l, &s).1];
            let mut d = Draw::default();
            let rest = kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 0.0);
            let mut quiet = Draw::default();
            quiet.level(-20.0);
            kick_frame(&mut quiet, &l, &s, &levels, 1.0 / 60.0, 0.0);
            quiet.level(-30.0);
            let hit = kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 1.0);
            assert_eq!(rest, hit, "{:?} {:?}", l.style, l.motion);
            assert!(hit.light == 0.0 && hit.swell == 0.0);
            let q = kick_frame(&mut quiet, &l, &s, &levels, 1.0, 0.0);
            assert_eq!(q.light, 0.0, "as faint as ever when quiet");
            assert!(!l.hears_beat(), "the beat is not even listened for");
        }
        assert!(Preset::Classic.look(&ThemeViz::default()).hears_beat() && Preset::Prism.look(&ThemeViz::default()).hears_beat());
    }

    #[test]
    fn a_kick_moves_bars_and_the_line_alike_at_any_frame_rate() {
        let s = stage(1400.0, 204.0);
        for style in [Style::Bars, Style::Line] {
            let l = look(style);
            let levels = vec![0.3; layout(&l, &s).1];
            let mut at = Vec::new();
            for fps in [30.0, 60.0, 144.0] {
                let mut d = Draw::default();
                let dt = 1.0 / fps;
                kick_frame(&mut d, &l, &s, &levels, dt, 1.0);
                let mut t = 0.0;
                while t + dt < 0.1 - 1e-4 {
                    kick_frame(&mut d, &l, &s, &levels, dt, 0.0);
                    t += dt;
                }
                // To 0.1 s after the kick exactly.
                kick_frame(&mut d, &l, &s, &levels, 0.1 - t, 0.0);
                at.push((d.kick, d.lit));
            }
            for (k, lit) in &at {
                assert!((k - at[0].0).abs() < 1e-3 && (lit - at[0].1).abs() < 1e-3, "{style:?}: {at:?}");
            }
            assert!(at[0].0 > 0.05 && at[0].0 < 0.2, "{style:?}: mostly gone in an eighth of a second: {}", at[0].0);
        }
    }

    #[test]
    fn a_still_frame_takes_no_kick() {
        let s = stage(1400.0, 204.0);
        for style in [Style::Bars, Style::Line] {
            let l = look(style);
            let levels = vec![0.3; layout(&l, &s).1];
            let rest = Draw::default().draw(&l, &s, true, &levels, &levels).clone();
            let mut d = Draw::default();
            d.pulse(Pulse { hit: 1.0, ..Pulse::default() });
            d.level(-30.0);
            assert_eq!(*d.draw(&l, &s, true, &levels, &levels), rest, "{style:?}");
        }
    }

    #[test]
    fn the_line_takes_the_kick_thickens_and_is_fuller_when_loud() {
        let l = Look { reflection: false, peaks: false, ..look(Style::Line) };
        let s = stage(1400.0, 204.0);
        let (n, bands) = layout(&l, &s);
        let levels = vec![0.3; bands];
        let mut d = Draw::default();
        let rest = points(&kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 0.0).line);
        let p = kick_frame(&mut d, &l, &s, &levels, 1.0 / 60.0, 1.0);
        let hit = points(&p.line);
        // The curve's points are every third (each segment's two control points before it).
        let ys = |v: &[(f32, f32)]| -> Vec<f32> { std::iter::once(v[0].1).chain(v.iter().skip(3).step_by(3).map(|p| p.1)).collect() };
        let (a, b) = (ys(&rest), ys(&hit));
        assert_eq!(a.len(), n);
        let rise: Vec<f32> = a.iter().zip(&b).map(|(a, b)| a - b).collect();
        let span = (bands as f32 * KICK_SPAN) as usize;
        assert!(rise[0] > 30.0 && rise[span / 2] < rise[0] && rise[span / 2] > 0.0, "{rise:?}");
        assert!(rise[span + 2..].iter().all(|&r| r.abs() < 0.05), "the low end only: {rise:?}");
        assert!((p.swell - 1.0).abs() < 1e-4 && p.light == 0.0, "twice as thick, its area as loud as it has been");
        // Fainter when the music is quiet against its loudest lately, as it was when loud.
        let mut e = Draw::default();
        e.level(0.0);
        kick_frame(&mut e, &l, &s, &levels, 1.0 / 60.0, 0.0);
        let loud = kick_frame(&mut e, &l, &s, &levels, 1.0 / 60.0, 0.0).light;
        e.level(-12.0);
        let mut quiet = 0.0;
        for _ in 0..600 {
            quiet = kick_frame(&mut e, &l, &s, &levels, 1.0 / 60.0, 0.0).light;
        }
        assert!(loud.abs() < 1e-3 && quiet < -0.25, "{loud} {quiet}");
    }


    thread_local! {
        /// The path text written as it was before `tenth`, with `write!` and `{:.1}`: to hold
        /// the text to what it was and to time the two (one thread's: tests run side by side).
        pub(super) static AS_BEFORE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn tenth_of(v: f32) -> String {
        let mut s = String::new();
        tenth(&mut s, v);
        s
    }

    #[test]
    fn a_number_is_written_as_format_writes_it_to_a_tenth_a_whole_one_without_its_point() {
        let as_fmt = |v: f32| {
            let t = format!("{v:.1}");
            let t = t.strip_suffix(".0").unwrap_or(&t);
            if t == "-0" { "0".to_string() } else { t.to_string() }
        };
        // Every hundredth out to ±3000 (the ties among them: x.x5 and the exact quarters), and
        // a million of any bits within a billion pixels.
        let mut vals: Vec<f32> = (-300_000..=300_000).map(|i| i as f32 * 0.01).collect();
        let mut seed = 0x2545_f491_u32;
        vals.extend((0..1_000_000).map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            f32::from_bits(seed)
        }));
        vals.extend([0.25, 0.75, -0.25, -0.75, 0.05, -0.05, 0.15, 0.35, 0.45, 9.95, 99.95, 999.95, -0.04, -0.0, 0.0, f32::MIN_POSITIVE, -1e-40, 16_777_217.0, 123_456_789.0, 1e9, -1e9]);
        for v in vals.into_iter().filter(|v| v.abs() <= 1e9) {
            assert_eq!(tenth_of(v), as_fmt(v), "{v:?}");
        }
        assert_eq!(tenth_of(0.25), "0.2", "a tie to the even tenth, as `{{:.1}}`");
        assert_eq!(tenth_of(0.75), "0.8");
        assert_eq!(tenth_of(-0.04), "0", "never -0");
        // What `{:.1}` would write and the parser reject (and so not draw the path at all):
        // a number it reads.
        for (v, want) in [(f32::NAN, "0"), (f32::INFINITY, "1000000000"), (f32::NEG_INFINITY, "-1000000000"), (f32::MAX, "1000000000"), (-3e9, "-1000000000")] {
            assert_eq!(tenth_of(v), want, "{v:?}");
        }
    }

    /// The path as Slint reads it from its text (`PathData::iter` in i-slint-core: lyon's SVG
    /// parser into lyon's path builder); text it rejects fails the test.
    fn parsed(text: &str) -> lyon_path::Path {
        let mut b = lyon_path::Path::builder();
        let r = lyon_extra::parser::PathParser::new().parse(&lyon_extra::parser::ParserOptions::DEFAULT, &mut lyon_extra::parser::Source::new(text.chars()), &mut b);
        assert!(r.is_ok(), "{r:?}: {text}");
        b.build()
    }

    /// Every path of a frame.
    fn texts(p: &Paths) -> impl Iterator<Item = &str> {
        [&p.fill, &p.caps, &p.line, &p.ripples, &p.rings, &p.sparks, &p.sparks_old, &p.sparks_glow, &p.front, &p.front_glow].into_iter().chain(&p.refl).map(String::as_str)
    }

    /// Frames of music: levels that move, peaks above them, a kick on every beat (two a
    /// second) and the tempo told.
    fn music(l: &Look, s: &Stage, frames: usize) -> Vec<Paths> {
        let (_, bands) = layout(l, s);
        let mut d = Draw::default();
        (0..frames)
            .map(|f| {
                let levels: Vec<f32> = (0..bands).map(|i| 0.15 + 0.7 * ((i as f32) * 0.23 + f as f32 * 0.11).sin().abs() * (1.0 - i as f32 / (bands as f32 * 1.6))).collect();
                let peaks: Vec<f32> = levels.iter().map(|v| v + 0.06).collect();
                let phase = (f as f32 / 30.0).fract();
                d.tick(1.0 / 60.0);
                d.level(-3.0);
                d.pulse(Pulse { hit: if f % 30 == 0 { 1.0 } else { 0.0 }, period: 0.5, phase, low: if f % 30 == 0 { GRID_WEAK.1 } else { 0.0 }, beat: f % 30 == 0, ..Pulse::default() });
                d.draw(l, s, true, &levels, &peaks).clone()
            })
            .collect()
    }

    /// Every preset on every kind of stage (the band, the tab, the page, the preview, a square
    /// cover's), through two seconds of music: the text as it is now reads back, path for path
    /// and point for point, as the text `write!` and `{:.1}` wrote.
    #[test]
    fn every_look_reads_back_as_the_path_it_was_written_as_before() {
        let t = ThemeViz::default();
        let stages = [stage(1840.0, 290.0), stage(388.0, 640.0), Stage { hub: 160.0, ..stage(1470.0, 922.0) }, stage(300.0, 160.0), Stage { hub: 120.0, corner: 14.0, ..stage(660.0, 520.0) }];
        for preset in Preset::ALL {
            let l = preset.look(&t);
            for s in &stages {
                AS_BEFORE.set(true);
                let before = music(&l, s, 120);
                AS_BEFORE.set(false);
                let now = music(&l, s, 120);
                for (a, b) in before.iter().zip(&now) {
                    for (a, b) in texts(a).zip(texts(b)) {
                        let (pa, pb) = (parsed(a), parsed(b));
                        if b.contains('c') && a.contains('A') {
                            // Dots, the small ones now curves (`dot`): the same dots, each
                            // starting where it did (`a_small_dot_is_two_curves...` for the rest).
                            let starts = |p: &lyon_path::Path| p.iter().filter_map(|e| if let lyon_path::Event::Begin { at } = e { Some(at) } else { None }).collect::<Vec<_>>();
                            assert_eq!(starts(&pa), starts(&pb), "{preset:?} on {s:?}:\n{a}\n{b}");
                            continue;
                        }
                        assert!(pa.iter().eq(pb.iter()), "{preset:?} on {s:?}:\n{a}\n{b}");
                        assert!(b.len() <= a.len(), "never longer");
                    }
                }
            }
        }
    }

    /// A dot up to `DOT_CUBIC` is two cubics, and every point of them within 0.05 px of the
    /// circle the numbers written stand for (the cubic's own error; the numbers are to a tenth,
    /// as an arc's were); a larger one is the two arcs it was.
    #[test]
    fn a_small_dot_is_two_curves_no_more_than_a_twentieth_of_a_pixel_from_its_circle() {
        use lyon_path::Event;
        // The curve itself, before any rounding: least out at `DOT_K`, by 1.32% of the radius.
        let out = (0..=4000).map(|i| i as f64 / 4000.0).map(|t| {
            let (m, k) = (1.0 - t, f64::from(DOT_K));
            let (x, y) = (-m * m * m - 3.0 * m * m * t + 3.0 * m * t * t + t * t * t, -3.0 * k * m * t);
            x.hypot(y) - 1.0
        });
        let (lo, hi) = out.fold((0.0f64, 0.0f64), |(lo, hi), e| (lo.min(e), hi.max(e)));
        assert!(lo > -0.01325 && hi < 0.01325 && f64::from(DOT_CUBIC) * hi.max(-lo) <= 0.05, "{lo} {hi}");
        // As written and read back: of the circle its own numbers give (the ends of the two
        // halves), every point of the curves within 0.05 px and a rounding of the control
        // points' height (a tenth's half, three quarters of which reaches the curve).
        let mut text = String::new();
        for i in 0..2000 {
            let (x, y, r) = (300.0 + i as f32 * 0.37, 200.0 - i as f32 * 0.213, 0.3 + (i % 70) as f32 * 0.05);
            text.clear();
            dot(&mut text, x, y, r);
            let path = parsed(&text);
            if r > DOT_CUBIC {
                assert!(text.contains('A') && !text.contains('c'), "{r}: {text}");
                continue;
            }
            let ev: Vec<_> = path.iter().collect();
            let [Event::Begin { at }, Event::Cubic { to: half, .. }, Event::Cubic { to: back, .. }, Event::End { close: true, .. }] = ev[..] else { panic!("{text}: {ev:?}") };
            assert!((at - back).length() < 1e-3, "{text}");
            let (cx, cy, rr) = ((at.x + half.x) / 2.0, at.y, (half.x - at.x) / 2.0);
            assert!((cx - x).abs() <= 0.051 && (cy - y).abs() <= 0.051 && (rr - r).abs() <= 0.051, "{text} for {x} {y} {r}");
            for e in &ev {
                let Event::Cubic { from, ctrl1, ctrl2, to } = *e else { continue };
                for u in (0..=32).map(|u| u as f32 / 32.0) {
                    let m = 1.0 - u;
                    let at = |a: f32, b: f32, c: f32, d: f32| m * m * m * a + 3.0 * m * m * u * b + 3.0 * m * u * u * c + u * u * u * d;
                    let d = (at(from.x, ctrl1.x, ctrl2.x, to.x) - cx).hypot(at(from.y, ctrl1.y, ctrl2.y, to.y) - cy) - rr;
                    assert!(d.abs() <= 0.05 + 0.0375 + 1e-3, "{text}: {d} px out at {u}");
                }
            }
        }
    }

    /// The path text's cost, written the way it was (`write!`, `{:.1}`) and the way it is
    /// (`tenth`), back to back: a frame's paths built (the whole of `Draw::draw`) and parsed
    /// as Slint parses them, for the default look behind the page and the halo, on the page's
    /// stage in a 1470 x 922 window. Prints nanoseconds a frame, the best of five rounds.
    ///
    ///     cargo test -p marquee-player --profile fast path_text_cost -- --ignored --nocapture
    #[test]
    #[ignore]
    fn path_text_cost() {
        use std::time::Instant;
        let t = ThemeViz::default();
        let s = Stage { hub: 160.0, refl_room: 90.0, ..stage(1470.0, 922.0) };
        let frames = 600;
        for preset in [Preset::Classic, Preset::Halo] {
            let l = preset.look(&t);
            let (_, bands) = layout(&l, &s);
            let input: Vec<(Vec<f32>, Pulse)> = (0..frames)
                .map(|f| {
                    let levels = (0..bands).map(|i| 0.15 + 0.7 * ((i as f32) * 0.23 + f as f32 * 0.11).sin().abs() * (1.0 - i as f32 / (bands as f32 * 1.6))).collect();
                    let kick = f % 30 == 0;
                    (levels, Pulse { hit: if kick { 1.0 } else { 0.0 }, period: 0.5, phase: (f as f32 / 30.0).fract(), low: if kick { GRID_WEAK.1 } else { 0.0 }, beat: kick, ..Pulse::default() })
                })
                .collect();
            // (build, parse, bytes) a frame, the best of five rounds, the old way and the new.
            let mut best = [(f64::MAX, f64::MAX, 0usize); 2];
            for _ in 0..5 {
                for (way, before) in [(0, true), (1, false)] {
                    AS_BEFORE.set(before);
                    let mut d = Draw::default();
                    let (mut build, mut parse, mut bytes) = (0u128, 0u128, 0usize);
                    for (levels, pulse) in &input {
                        d.tick(1.0 / 60.0);
                        d.level(-3.0);
                        d.pulse(*pulse);
                        let t0 = Instant::now();
                        let p = d.draw(&l, &s, true, levels, levels);
                        build += t0.elapsed().as_nanos();
                        let t1 = Instant::now();
                        for text in texts(p).filter(|t| !t.is_empty()) {
                            let mut b = lyon_path::Path::builder();
                            let _ = lyon_extra::parser::PathParser::new().parse(&lyon_extra::parser::ParserOptions::DEFAULT, &mut lyon_extra::parser::Source::new(text.chars()), &mut b);
                            std::hint::black_box(b.build());
                        }
                        parse += t1.elapsed().as_nanos();
                        bytes += texts(p).map(str::len).sum::<usize>();
                    }
                    let n = frames as f64;
                    let b = &mut best[way];
                    *b = (b.0.min(build as f64 / n), b.1.min(parse as f64 / n), bytes / frames);
                }
            }
            AS_BEFORE.set(false);
            let [(b0, p0, n0), (b1, p1, n1)] = best;
            println!("{preset:?}: build {b0:.0} -> {b1:.0} ns a frame, parse {p0:.0} -> {p1:.0} ns, both {:.0} -> {:.0} ns ({:.0}% less); {n0} -> {n1} bytes", b0 + p0, b1 + p1, 100.0 * (1.0 - (b1 + p1) / (b0 + p0)));
        }
    }

    #[test]
    fn a_line_in_passes_shows_as_strong_as_its_pixel_is_covered() {
        // n of k passes over a pixel lay down n/k of the brush's alpha, whatever that alpha
        // (to the 8 bits a colour holds), and all k of them the brush itself.
        let alpha = |b: &Brush| f32::from(b.color().alpha()) / 255.0;
        for k in [2, 3, 4] {
            for a in [1.0, 0.75, 0.36, 0.1] {
                let line = Brush::SolidColor(colour([200, 80, 220], a));
                let passes = feather_passes(&line, k);
                assert_eq!(passes.len(), k);
                let mut through = 1.0;
                for (n, p) in passes.iter().enumerate() {
                    through *= 1.0 - alpha(p);
                    let shown = 1.0 - through;
                    let want = alpha(&line) * (n + 1) as f32 / k as f32;
                    assert!((shown - want).abs() < 2.5 / 255.0, "k {k} a {a} pass {n}: {shown} where {want}");
                    assert_eq!(p.color().with_alpha(1.0), line.color().with_alpha(1.0));
                }
            }
        }
    }

    #[test]
    fn the_passes_keep_their_brush_s_kind_and_geometry_each_stop_its_own_alpha() {
        let stops = || [stop(0.0, [255, 0, 0], 1.0), stop(0.5, [0, 255, 0], 0.5), stop(1.0, [0, 0, 255], 0.75)];
        let brushes = [
            Brush::LinearGradient(LinearGradientBrush::new(90.0, stops())),
            Brush::RadialGradient(RadialGradientBrush::new_circle(stops()).with_center(120.0, 80.0).with_radius(300.0)),
            Brush::RadialGradient(RadialGradientBrush::new_circle(stops())),
            Brush::ConicGradient(ConicGradientBrush::new(0.0, stops()).with_center(120.0, 80.0)),
            Brush::ConicGradient(ConicGradientBrush::new(0.0, stops())),
        ];
        for b in &brushes {
            // Alpha as it was: the very brush back.
            assert_eq!(&map_alpha(b, |a| a), b);
            // The last of two passes: each stop's own (a stop at alpha 1 stays at 1, one at
            // 0.5 takes the 1/4 laid down to 1/2: 1/3).
            let last = &feather_passes(b, 2)[1];
            let a = |b: &Brush| -> Vec<u8> {
                match b {
                    Brush::LinearGradient(g) => g.stops().map(|s| s.color.alpha()).collect(),
                    Brush::RadialGradient(g) => g.stops().map(|s| s.color.alpha()).collect(),
                    Brush::ConicGradient(g) => g.stops().map(|s| s.color.alpha()).collect(),
                    _ => unreachable!(),
                }
            };
            assert_eq!(a(last), vec![255, 85, 153], "{b:?}");
            assert_eq!(&map_alpha(last, |_| 1.0), &map_alpha(b, |_| 1.0));
        }
    }
}
