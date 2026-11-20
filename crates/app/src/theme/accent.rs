//! The UI accent follows the playing track's art.
//!
//! The colour is extracted once per cover and cached next to the art
//! (`library::accent`, `ArtCache::accent`), read on a worker thread, made usable on the
//! dark theme (`accent::usable`), and eased in over `[accent] transition_ms` by stepping
//! through OKLCH here, since Slint's own colour animation mixes in sRGB and passes
//! through grey between complementary hues. Nothing playing or no usable colour in the
//! art: `[accent] fallback`. `[accent] source = "fixed"` pins `[colors] accent` instead.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use library::{ArtCache, accent};
use slint::{Color, ComponentHandle, Timer, TimerMode};

use crate::{AppWindow, Theme};

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Follow the art; false pins `fixed`.
    pub follow_art: bool,
    pub fixed: ([u8; 3], [u8; 3]),
    pub fallback: [u8; 3],
    pub transition: Duration,
}

struct Anim {
    app: slint::Weak<AppWindow>,
    config: Config,
    /// The colour from the art of what is playing, if any (raw, before `usable`).
    art: Option<[u8; 3]>,
    shown: [u8; 3],
    from: [u8; 3],
    to: [u8; 3],
    started: Instant,
    timer: Timer,
    /// Bumped per request, so a slow worker answer for an old track is dropped.
    generation: u64,
}

thread_local! {
    static STATE: RefCell<Option<Anim>> = const { RefCell::new(None) };
}

fn to_color([r, g, b]: [u8; 3]) -> Color {
    Color::from_rgb_u8(r, g, b)
}

pub fn install(app: &AppWindow, config: Config) {
    let start = if config.follow_art { accent::usable(config.fallback).0 } else { config.fixed.0 };
    STATE.with(|s| {
        *s.borrow_mut() = Some(Anim {
            app: app.as_weak(),
            config,
            art: None,
            shown: start,
            from: start,
            to: start,
            started: Instant::now(),
            timer: Timer::default(),
            generation: 0,
        });
    });
    retarget(true);
}

/// The theme changed (hot reload).
pub fn set_config(config: Config) {
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(a) = s.as_mut() else { return false };
        let changed = a.config != config;
        a.config = config;
        changed
    });
    if changed {
        retarget(true);
    }
}

/// The playing track changed: follow its art (`None`: nothing playing or no art).
pub fn follow(art_cache: &ArtCache, stem: Option<String>) {
    let generation = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let a = s.as_mut()?;
        a.generation += 1;
        Some(a.generation)
    });
    let Some(generation) = generation else { return };
    let Some(stem) = stem else {
        apply_art(generation, None);
        return;
    };
    let cache = art_cache.clone();
    std::thread::spawn(move || {
        let colour = cache.accent(&stem);
        let _ = slint::invoke_from_event_loop(move || apply_art(generation, colour));
    });
}

fn apply_art(generation: u64, colour: Option<[u8; 3]>) {
    let fresh = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(a) = s.as_mut() else { return false };
        if a.generation != generation || a.art == colour {
            return false;
        }
        a.art = colour;
        true
    });
    if fresh {
        retarget(false);
    }
}

/// Start easing toward the colour the config and art call for.
fn retarget(instant: bool) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(a) = s.as_mut() else { return };
        let target = if a.config.follow_art {
            accent::usable(a.art.unwrap_or(a.config.fallback)).0
        } else {
            a.config.fixed.0
        };
        if instant || a.config.transition.is_zero() {
            a.shown = target;
            a.from = target;
            a.to = target;
            a.timer.stop();
            paint(a);
            return;
        }
        if target == a.to {
            return;
        }
        a.from = a.shown;
        a.to = target;
        a.started = Instant::now();
        a.timer.start(TimerMode::Repeated, Duration::from_millis(16), tick);
    });
}

fn tick() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(a) = s.as_mut() else { return };
        let t = a.started.elapsed().as_secs_f32() / a.config.transition.as_secs_f32().max(0.001);
        // Ease in-out (smoothstep): a slow drift, no visible start or stop.
        let e = t.clamp(0.0, 1.0);
        let e = e * e * (3.0 - 2.0 * e);
        a.shown = accent::mix(a.from, a.to, e);
        if t >= 1.0 {
            a.shown = a.to;
            a.timer.stop();
        }
        paint(a);
    });
}

fn paint(a: &Anim) {
    let Some(app) = a.app.upgrade() else { return };
    let theme = app.global::<Theme>();
    theme.set_accent(to_color(a.shown));
    // Text on the accent: the fixed pin keeps its own; otherwise the same hue, very dark.
    let text = if a.config.follow_art { accent::usable(a.shown).1 } else { a.config.fixed.1 };
    theme.set_accent_text(to_color(text));
    // The visualizer's brushes take the accent from here (its Accent and Cover colours).
    crate::visualizer::accent_changed();
}
