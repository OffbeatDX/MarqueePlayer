//! The tray icon moves while music plays, and is the static glyph otherwise.
//!
//! The icon is the app's sound wave (`tray/icon_art.rs`): five rounded bars in one amber to
//! pink gradient. While music plays, each bar bobs on its own two slow, unrelated rhythms
//! (`wave`), like an equalizer, so whichever bar is tallest keeps changing and the pattern
//! never visibly repeats. It does not follow the music (no audio is read for it), which
//! keeps it next to free. It runs at `[tray] fps` (5 a second by default). When playback starts the
//! bars blend from the resting glyph into the wave over `BLEND`, and when it pauses, stops
//! or ends they ease back to the resting heights over the same time; then the timer stops
//! and the icon is the static glyph again (the same pixels: one drawing path, the resting
//! heights). Nothing runs for it while paused or stopped.
//!
//! Each frame is drawn at the notification area's own icon size (the small icon size at
//! the system DPI: 16 px at 100 %, 24 at 150 %, 32 at 200 %), not scaled down from a big
//! one, and handed to Slint's tray, which makes a new HICON from it (a DIB section and a
//! mask, both deleted at once), replaces the shown one with `NIM_MODIFY` and destroys the
//! old one. Each new icon also makes the shell redraw the notification area (in
//! explorer.exe), so a frame in which no bar has moved by a third of a pixel since the
//! last icon sent is not sent at all. (That is Windows; on Linux the frames go to the
//! StatusNotifierItem through ksni, `platform::linux::tray`.)

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use player_core::PlayState;
use slint::{Timer, TimerMode};

use crate::tray::icon::{self, REST_HEIGHTS as REST};
use crate::state::{self, State};

pub const BARS: usize = 5;
/// The shortest and tallest a bar gets while playing, in the glyph's 256-unit square (at
/// rest they are 56 to 172; a bar is 24 wide, so the shortest is still a short pill).
const LOW: f32 = 34.0;
const HIGH: f32 = 176.0;
/// Seconds to blend from the resting glyph into the music when playback starts, and back
/// when it stops.
pub const BLEND: f32 = 0.6;

/// A bar's height for its level (0 to 1).
pub fn height(level: f32) -> f32 {
    LOW + (HIGH - LOW) * level.clamp(0.0, 1.0)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Each bar's two rhythms (cycles per second): slow, and unrelated to each other and to
/// the other bars', so the bars move independently and the pattern never visibly repeats.
const RHYTHMS: [(f32, f32); BARS] = [(0.27, 0.53), (0.36, 0.43), (0.31, 0.59), (0.4, 0.47), (0.23, 0.56)];
/// Where each bar starts in its rhythms, so they do not all rise together.
const PHASES: [(f32, f32); BARS] = [(0.0, 0.3), (0.45, 0.8), (0.2, 0.1), (0.7, 0.55), (0.9, 0.35)];

/// The bars' levels (0 to 1), `t` seconds in: each bar bobs on its own two rhythms, like an
/// equalizer, so whichever is tallest changes as it goes.
pub fn wave(t: f32) -> [f32; BARS] {
    use std::f32::consts::TAU;
    let mut l = [0.0; BARS];
    for (i, v) in l.iter_mut().enumerate() {
        let ((f, g), (p, q)) = (RHYTHMS[i], PHASES[i]);
        let a = (TAU * (f * t + p)).sin();
        let b = (TAU * (g * t + q)).sin();
        *v = (0.5 + 0.3 * a + 0.2 * b).clamp(0.0, 1.0);
    }
    l
}

/// The bars' motion: the music's heights, blended with the resting glyph by `mix`.
#[derive(Clone, Debug)]
pub struct Anim {
    mix: f32,
    music: [f32; BARS],
}

impl Default for Anim {
    fn default() -> Self {
        Self { mix: 0.0, music: REST }
    }
}

impl Anim {
    /// One frame, `dt` seconds after the last. `levels`: the bars' levels while music plays;
    /// `None` when it does not (paused, stopped, ended, the animation turned off): the bars
    /// ease back from where they were. Returns the heights to draw and whether another
    /// frame is wanted (false once at rest and not playing: stop).
    pub fn step(&mut self, levels: Option<&[f32]>, dt: f32) -> ([f32; BARS], bool) {
        let dt = dt.clamp(0.0, 0.25);
        match levels {
            Some(l) => {
                for (i, m) in self.music.iter_mut().enumerate() {
                    *m = height(l.get(i).copied().unwrap_or(0.0));
                }
                self.mix = (self.mix + dt / BLEND).min(1.0);
            }
            // The music's last heights stay where they were: each bar moves straight back.
            None => self.mix = (self.mix - dt / BLEND).max(0.0),
        }
        if self.mix <= 0.0 {
            return (REST, levels.is_some());
        }
        let e = smoothstep(self.mix);
        let mut h = REST;
        for (i, v) in h.iter_mut().enumerate() {
            *v += (self.music[i] - REST[i]) * e;
        }
        (h, true)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn at_rest(&self) -> bool {
        self.mix <= 0.0
    }
}

/// The tray icon's size here (Windows: the notification area's small icon at the system
/// DPI; Linux: what a StatusNotifierItem host shows, `platform::system::tray_icon_size`).
pub fn icon_size() -> u32 {
    crate::platform::system::tray_icon_size()
}

/// The static glyph at the tray's size.
pub fn static_icon() -> slint::Image {
    let size = icon_size();
    icon::image_from_rgba(&icon::tray_frame(size, &REST), size)
}

struct TrayAnim {
    timer: Timer,
    anim: Anim,
    /// Seconds into the loop.
    clock: f32,
    last: Option<Instant>,
    size: u32,
    /// The pixels last handed to the tray, and the heights they were drawn from.
    shown: Vec<u8>,
    shown_heights: [f32; BARS],
    /// For the bench: every frame's heights, and (start, time taken, sent to the tray).
    stats: Option<Vec<FrameStat>>,
    sent: u64,
}

thread_local! {
    static ANIM: RefCell<Option<TrayAnim>> = const { RefCell::new(None) };
    static FPS: Cell<u32> = const { Cell::new(5) };
}

pub fn install() {
    let size = icon_size();
    log::info!("tray icon: {size} px");
    ANIM.with(|a| {
        *a.borrow_mut() = Some(TrayAnim {
            timer: Timer::default(),
            clock: 0.0,
            anim: Anim::default(),
            last: None,
            size,
            shown: icon::tray_frame(size, &REST),
            shown_heights: REST,
            stats: None,
            sent: 0,
        });
    });
}

/// theme.toml `[tray] fps`; applies from the next start.
pub fn set_fps(fps: u32) {
    FPS.with(|f| f.set(fps.clamp(4, 30)));
}

fn fps() -> u32 {
    FPS.with(Cell::get)
}

fn playing(st: &Rc<State>) -> bool {
    st.settings.borrow().tray_animation && st.player.status().state == PlayState::Playing
}

/// Playback or the setting may have changed: start the animation if music plays. Cheap;
/// called with every tray update (`sync_tray`, each playback tick).
pub fn wake(st: &Rc<State>) {
    if !crate::has_tray() {
        return;
    }
    let on = playing(st);
    let off = !st.settings.borrow().tray_animation;
    ANIM.with(|a| {
        let mut a = a.borrow_mut();
        let Some(t) = a.as_mut() else { return };
        if on && !t.timer.running() {
            let size = icon_size();
            if size != t.size {
                t.size = size;
                t.shown.clear();
                t.shown_heights = [-1000.0; BARS];
            }
            t.last = None;
            let every = Duration::from_millis(u64::from(1000 / fps()));
            t.timer.start(TimerMode::Repeated, every, || state::with(frame).unwrap_or(()));
        } else if off && t.timer.running() {
            // Turned off: the static glyph at once.
            stop(t);
        }
    });
}

fn stop(t: &mut TrayAnim) {
    t.timer.stop();
    t.clock = 0.0;
    t.anim = Anim::default();
    t.last = None;
    show(t, &REST, true);
}

/// The smallest move worth a new icon, in icon pixels: each one makes the shell redraw
/// the notification area, so a frame where no bar moves by a third of a pixel is skipped.
const MIN_MOVE_PX: f32 = 0.34;

/// Hand the frame to the tray, unless it looks (nearly) the same as the one there.
/// `exact`: the resting glyph, sent whenever its pixels differ.
fn show(t: &mut TrayAnim, heights: &[f32; BARS], exact: bool) -> bool {
    // Glyph units per icon pixel: the 256 square at the glyph's scale (1.42).
    let unit = 256.0 / 1.42 / t.size as f32;
    if !exact && heights.iter().zip(&t.shown_heights).all(|(a, b)| (a - b).abs() < MIN_MOVE_PX * unit) {
        return false;
    }
    let rgba = icon::tray_frame(t.size, heights);
    if rgba == t.shown {
        return false;
    }
    crate::set_tray_image(icon::image_from_rgba(&rgba, t.size));
    t.shown = rgba;
    t.shown_heights = *heights;
    t.sent += 1;
    true
}

fn frame(st: &Rc<State>) {
    crate::wakes::note(&crate::wakes::TRAY_FRAMES);
    let on = playing(st);
    ANIM.with(|a| {
        let mut a = a.borrow_mut();
        let Some(t) = a.as_mut() else { return };
        let started = Instant::now();
        let every = 1.0 / fps() as f32;
        let dt = t.last.map_or(every, |l| (started - l).as_secs_f32());
        t.last = Some(started);
        let levels = on.then(|| {
            // Wrapped long before f32 loses the precision the rhythms need.
            t.clock = (t.clock + dt.min(0.25)) % 3600.0;
            wave(t.clock)
        });
        let (h, more) = t.anim.step(levels.as_ref().map(|l| &l[..]), dt);
        let sent = show(t, &h, false);
        if let Some(s) = t.stats.as_mut() {
            s.push((started, started.elapsed(), sent, h));
        }
        if !more {
            stop(t);
        }
    });
}

#[cfg(feature = "bench")]
/// The bench: start or stop recording frames; `take_stats` returns them.
pub fn record(on: bool) {
    ANIM.with(|a| {
        if let Some(t) = a.borrow_mut().as_mut() {
            t.stats = on.then(Vec::new);
        }
    });
}

pub type FrameStat = (Instant, Duration, bool, [f32; BARS]);

#[cfg(feature = "bench")]
pub fn take_stats() -> Vec<FrameStat> {
    ANIM.with(|a| a.borrow_mut().as_mut().and_then(|t| t.stats.take()).unwrap_or_default())
}

#[cfg(feature = "bench")]
/// The bench: (timer running, icons sent so far, icon size).
pub fn running() -> (bool, u64, u32) {
    ANIM.with(|a| a.borrow().as_ref().map_or((false, 0, 0), |t| (t.timer.running(), t.sent, t.size)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 5.0;

    #[test]
    fn levels_map_to_heights_inside_the_glyph() {
        assert_eq!(height(0.0), LOW);
        assert_eq!(height(1.0), HIGH);
        assert_eq!(height(2.0), HIGH);
        assert_eq!(height(-1.0), LOW);
        assert!(height(0.5) > LOW && height(0.5) < HIGH);
        // The glyph's scale (1.42) keeps the tallest bar inside the 256 square.
        const { assert!(HIGH * 1.42 <= 256.0) };
    }

    #[test]
    fn starts_from_the_glyph_and_blends_into_the_wave() {
        let mut a = Anim::default();
        let loud = [1.0; BARS];
        let (h, more) = a.step(Some(&loud), DT);
        assert!(more);
        // One frame in: moving from the resting heights toward the wave, not jumping.
        for i in 0..BARS {
            assert!(h[i] != REST[i] || REST[i] == HIGH);
            assert!((h[i] - REST[i]).abs() < (HIGH - REST[i]).abs() * 0.5 + 0.01, "bar {i}: {}", h[i]);
        }
        for _ in 0..10 {
            a.step(Some(&loud), DT);
        }
        let l = wave(1.3);
        let (h, _) = a.step(Some(&l), DT);
        assert!(h.iter().zip(l.map(height)).all(|(a, b)| (a - b).abs() < 1e-3), "fully the wave once blended in");
    }

    #[test]
    fn the_bars_move_independently_and_each_is_tallest_at_times() {
        let mut tallest = [0; BARS];
        let mut t = 0.0;
        let mut prev = wave(0.0);
        let mut moved = 0;
        while t < 60.0 {
            t += DT;
            let l = wave(t);
            assert!(l.iter().all(|v| (0.0..=1.0).contains(v)));
            tallest[(0..BARS).max_by(|&i, &j| l[i].total_cmp(&l[j])).unwrap()] += 1;
            // Smooth at 5 fps: no bar jumps more than a third of its range in a frame.
            assert!(l.iter().zip(prev).all(|(a, b)| (a - b).abs() < 0.34), "{t}: {prev:?} -> {l:?}");
            moved += l.iter().zip(prev).filter(|(a, b)| (*a - *b).abs() > 0.02).count();
            prev = l;
        }
        // Every bar leads for a fair share of the time.
        assert!(tallest.iter().all(|&n| n > 15), "{tallest:?}");
        assert!(moved > 900, "{moved}");
    }

    #[test]
    fn settles_on_the_resting_glyph_over_a_few_frames_then_stops() {
        let mut a = Anim::default();
        for _ in 0..12 {
            a.step(Some(&[0.9, 0.1, 0.0, 1.0, 0.6]), DT);
        }
        let mut frames = 0;
        let mut prev = a.step(None, 0.0).0;
        loop {
            let (h, more) = a.step(None, DT);
            frames += 1;
            // Each bar moves straight home: never overshoots, never turns back.
            for i in 0..BARS {
                let (before, now) = (prev[i] - REST[i], h[i] - REST[i]);
                assert!(now.abs() <= before.abs() + 1e-3 && now * before >= -1e-3, "bar {i}: {before} -> {now}");
            }
            prev = h;
            if !more {
                assert_eq!(h, REST, "the last frame is exactly the static glyph");
                break;
            }
            assert!(frames < 20);
        }
        // A few frames, not a snap and not a long fade (0.6 s at 5 fps: 3).
        assert!((3..=5).contains(&frames), "{frames} frames");
        assert!(a.at_rest());
        // At rest and still not playing: nothing more to do.
        assert_eq!(a.step(None, DT), (REST, false));
    }

    #[test]
    fn a_pause_mid_blend_goes_back_from_where_it_is() {
        let mut a = Anim::default();
        a.step(Some(&[1.0; BARS]), DT);
        let (h, more) = a.step(None, DT);
        assert!(!more || h != REST);
        let (h, more) = a.step(None, DT);
        assert_eq!((h, more), (REST, false));
    }

    #[test]
    fn the_static_glyph_and_the_rest_frame_are_the_same_pixels() {
        for size in [16, 20, 24, 32] {
            assert_eq!(icon::tray_frame_on(size, &REST, false), crate::tray::icon::tray_rgba(size), "{size} px");
        }
    }
}
