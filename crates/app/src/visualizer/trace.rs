//! The halo over a stretch of real sound, frame by frame, for looking at what cannot be
//! watched (an agent has no eyes on the window while it moves): what the beat tracker tells,
//! where every ripple and ring is (lines of JSON, `trace.jsonl`), and the frames of chosen
//! moments as SVG (`f-<frame>.svg`: the frame's own paths under its own brushes, so what is
//! too faint to see there is too faint in the window).
//!
//!     ffmpeg -i track.flac -f f32le -ac 1 -ar 48000 track.pcm
//!     MP_TRACE_PCM=track.pcm MP_TRACE_OUT=<folder> cargo test -p marquee-player halo_trace -- --ignored
//!
//! `MP_TRACE_FRAMES=12-12.5,20-20.4`: the seconds whose frames are drawn (every frame in them).
//! `MP_TRACE_STAGE=1300x1300x340`: the stage's width, height and hub. `MP_TRACE_STRENGTH=0.85`:
//! the look's opacity where it shows. `MP_HALO_WITHOUT` applies as in the app.

use super::*;
use crate::visualizer::beat::Tracker;
use crate::visualizer::look::{Preset, ThemeViz, analysis};
use player_core::spectrum::Analyzer;

/// A brush as an SVG paint: a radial gradient as one (the ring's are centred on the cover,
/// out to the reach), anything else as its middle colour.
fn paint(defs: &mut String, id: &str, b: &Brush, g: &Halo) -> String {
    let rgba = |c: Color| format!("rgba({},{},{},{:.3})", c.red(), c.green(), c.blue(), f32::from(c.alpha()) / 255.0);
    match b {
        Brush::SolidColor(c) => rgba(*c),
        Brush::RadialGradient(r) => {
            let _ = write!(defs, "<radialGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\">", g.cx, g.cy, g.hub + g.gap + g.travel + 1.0);
            for s in r.stops() {
                let _ = write!(defs, "<stop offset=\"{:.4}\" stop-color=\"rgb({},{},{})\" stop-opacity=\"{:.3}\"/>", s.position, s.color.red(), s.color.green(), s.color.blue(), f32::from(s.color.alpha()) / 255.0);
            }
            defs.push_str("</radialGradient>");
            format!("url(#{id})")
        }
        Brush::ConicGradient(c) => {
            let stops: Vec<_> = c.stops().collect();
            stops.get(stops.len() / 4).map_or("none".into(), |s| rgba(s.color))
        }
        Brush::LinearGradient(l) => l.stops().next().map_or("none".into(), |s| rgba(s.color)),
        _ => "none".into(),
    }
}

/// A frame as the window would show it, cropped to the halo's reach.
fn svg(id: usize, p: &Paths, b: &Brushes, g: &Halo) -> String {
    let mut defs = String::new();
    let reach = g.hub + g.gap + g.travel + 12.0;
    let mut body = String::new();
    let fill = |name: &str, d: &str, brush: &Brush, defs: &mut String, body: &mut String| {
        if !d.is_empty() {
            let paint = paint(defs, &format!("{name}{id}"), brush, g);
            let _ = write!(body, "<path d=\"{d}\" fill=\"{paint}\"/>");
        }
    };
    let stroke = |name: &str, d: &str, brush: &Brush, width: f32, defs: &mut String, body: &mut String| {
        if !d.is_empty() {
            let paint = paint(defs, &format!("{name}{id}"), brush, g);
            let _ = write!(body, "<path d=\"{d}\" fill=\"none\" stroke=\"{paint}\" stroke-width=\"{width:.2}\" stroke-linejoin=\"round\"/>");
        }
    };
    let bloom = paint(&mut defs, &format!("bloom{id}"), &b.bloom, g);
    let _ = write!(body, "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{reach:.1}\" fill=\"{bloom}\" opacity=\"{:.3}\"/>", g.cx, g.cy, 0.4 + 0.6 * p.pulse.clamp(0.0, 1.0));
    fill("fill", &p.fill, &b.fill, &mut defs, &mut body);
    stroke("glow", &p.line, &b.glow, b.glow_width * (1.0 + p.glow), &mut defs, &mut body);
    stroke("line", &p.line, &b.line, b.line_width, &mut defs, &mut body);
    stroke("ripples", &p.ripples, &b.ripples, b.ripple_width, &mut defs, &mut body);
    stroke("rings", &p.rings, &b.rings, b.ring_width, &mut defs, &mut body);
    fill("sparksglow", &p.sparks_glow, &b.sparks_glow, &mut defs, &mut body);
    fill("sparksold", &p.sparks_old, &b.sparks_old, &mut defs, &mut body);
    fill("sparks", &p.sparks, &b.sparks, &mut defs, &mut body);
    let _ = write!(body, "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"#1a1626\" stroke=\"#2a2438\"/>", g.cx, g.cy, g.hub * (1.0 + 0.025 * p.punch.clamp(0.0, 1.0)));
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{:.1} {:.1} {:.1} {:.1}\"><defs>{defs}</defs><rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"#0c0b12\"/>{body}</svg>",
        g.cx - reach,
        g.cy - reach,
        reach * 2.0,
        reach * 2.0,
        g.cx - reach,
        g.cy - reach,
        reach * 2.0,
        reach * 2.0
    )
}

#[test]
#[ignore = "a tool, not a check: see this file's comment"]
fn halo_trace() {
    let Ok(pcm) = std::env::var("MP_TRACE_PCM") else { return };
    let out = std::path::PathBuf::from(std::env::var("MP_TRACE_OUT").expect("MP_TRACE_OUT=<folder>"));
    std::fs::create_dir_all(&out).unwrap();
    let samples: Vec<f32> = std::fs::read(&pcm).unwrap().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    let rate = 48_000u32;
    let dims: Vec<f32> = std::env::var("MP_TRACE_STAGE").unwrap_or_else(|_| "1300x1300x340".into()).split('x').filter_map(|v| v.parse().ok()).collect();
    let stage = Stage { hub: dims[2], ..Stage::new(dims[0], dims[1], (10.0, 4.0)) };
    let windows: Vec<(f32, f32)> = std::env::var("MP_TRACE_FRAMES")
        .unwrap_or_default()
        .split(',')
        .filter_map(|w| w.split_once('-'))
        .filter_map(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)))
        .collect();
    let strength: f32 = std::env::var("MP_TRACE_STRENGTH").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let look = Preset::Halo.look(&ThemeViz::default());
    let palette = [[0x8a, 0x4f, 0xff], [0x2c, 0xd5, 0xe0], [0xff, 0x3d, 0xa5]];
    let stops = base_stops(&look, palette[0], &palette);
    let g = halo(&stage);
    let (_, bands) = layout(&look, &stage);
    let mut analyzer = Analyzer::new(analysis(&look));
    let mut tracker = Tracker::default();
    let mut d = Draw::default();
    let n = Analyzer::fft_size(rate);
    let dt = 1.0 / 60.0;
    let mut window = vec![0.0f32; n];
    let mut lines = String::new();
    let (most, life) = ripple_room(&g);
    let _ = writeln!(lines, "{{\"stage\":[{},{}],\"hub\":{:.1},\"len\":{:.1},\"travel\":{:.1},\"most\":{most},\"life\":{life:.3},\"ring_reach\":{RING_REACH}}}", stage.w, stage.h, g.hub, g.len, g.travel);
    for f in 1..(samples.len() as f32 / rate as f32 / dt) as usize {
        let t = f as f32 * dt;
        let end = (t * rate as f32) as usize;
        for (i, s) in window.iter_mut().enumerate() {
            *s = (end + i).checked_sub(n).and_then(|at| samples.get(at)).copied().unwrap_or(0.0);
        }
        analyzer.update(Some((&window, rate)), bands, dt);
        let heard = tracker.update(&window, rate, dt);
        d.pulse(Pulse { hit: heard.hit, snap: heard.snap, period: heard.period, bright: heard.bright, beat: heard.beat, phase: heard.phase, low: heard.low });
        d.tick(dt);
        d.level(analyzer.level_slide_db());
        let (lv, pk) = (analyzer.levels().to_vec(), analyzer.peaks().to_vec());
        d.draw(&look, &stage, true, &lv, &pk);
        let low = (bands / 8).max(1);
        let swell = lv[..low].iter().sum::<f32>() / low as f32;
        let _ = write!(lines, "{{\"t\":{t:.4},\"hit\":{:.3},\"kick\":{},\"snap\":{:.3},\"bpm\":{:.1},\"swell\":{swell:.3},\"punch\":{:.3},\"bright\":{:.4},\"beat\":{},\"phase\":{:.3},\"low\":{:.2},\"rip\":[", heard.hit, heard.kick, heard.snap, heard.bpm(), d.paths.punch, heard.bright, heard.beat, heard.phase, heard.low);
        for (i, r) in d.ripples.list.iter().enumerate() {
            let _ = write!(lines, "{}[{:.4},{:.3},{},{:.3}]", if i > 0 { "," } else { "" }, r.far, r.throw, u8::from(r.leaving), r.age);
        }
        lines.push_str("],\"rings\":[");
        for (i, r) in d.ripples.rings.iter().filter(|r| r.shown).enumerate() {
            let _ = write!(lines, "{}[{:.4},{:.3}]", if i > 0 { "," } else { "" }, r.far(), r.age);
        }
        lines.push_str("]}\n");
        if windows.iter().any(|&(a, b)| t >= a && t < b) {
            let b = brushes(&look, &stage, d.paths.top_h, d.paths.refl_h, &stops, strength);
            std::fs::write(out.join(format!("f-{f:05}.svg")), svg(f, &d.paths, &b, &g)).unwrap();
        }
    }
    std::fs::write(out.join("trace.jsonl"), lines).unwrap();
}

/// The dots of a path of them (`dot`): each one's centre and radius, in order.
fn dot_list(path: &str) -> Vec<(f32, f32, f32)> {
    path.split('M')
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let v: Vec<f32> = s.split([' ', 'A', 'Z']).filter_map(|n| n.parse().ok()).collect();
            (v.len() >= 4).then(|| (v[0] + v[2], v[1], v[2]))
        })
        .collect()
}

/// A brush as JSON: its kind and stops (a radial one centred on the cover, out to the reach;
/// a conic one round the cover from the top, clockwise).
fn brush_json(b: &Brush) -> String {
    let stop = |s: GradientStop| format!("[{:.4},{},{},{},{:.4}]", s.position, s.color.red(), s.color.green(), s.color.blue(), f32::from(s.color.alpha()) / 255.0);
    let (kind, stops): (&str, Vec<String>) = match b {
        Brush::SolidColor(c) => ("solid", vec![stop(GradientStop { color: *c, position: 0.0 })]),
        Brush::RadialGradient(r) => ("radial", r.stops().cloned().map(stop).collect()),
        Brush::ConicGradient(c) => ("conic", c.stops().cloned().map(stop).collect()),
        Brush::LinearGradient(l) => ("linear", l.stops().cloned().map(stop).collect()),
        _ => ("none", Vec::new()),
    };
    format!("{{\"kind\":\"{kind}\",\"stops\":[{}]}}", stops.join(","))
}

/// Saturn over made-up music, frame by frame at 60 a second, for an animation of it to be
/// looked at: every grain's place, size and layer in each frame (`<tag>-<case>.bin`: frames
/// and grains as u32, then a frame each: the beat, pulse, punch and phase as bytes, and six
/// bytes a grain, x and y in quarter pixels, the radius in twentieths, the layer: 0 not
/// drawn, 1 behind the cover, 2 in front, 3 in front and glowing), and the brushes and the
/// stage (`saturn.json`). The grains are told apart by their order in each layer's path (the
/// ring's order) and which of them are drawn (the fullness drops them in a fixed order).
///
///     MP_TRACE_OUT=<folder> MP_TRACE_TAG=after cargo test -p marquee-player saturn_trace -- --ignored
///
/// The cases: 120 a minute, a kick on every beat; 170 a minute; 72 a minute with soft beats (the
/// low notes' onset only); 120 a minute going quiet for four seconds (no kick, 12 dB down).
#[test]
#[ignore = "a tool, not a check: see its comment"]
fn saturn_trace() {
    let Ok(out) = std::env::var("MP_TRACE_OUT") else { return };
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();
    let tag = std::env::var("MP_TRACE_TAG").unwrap_or_else(|_| "now".into());
    // Behind the page: a cover drawn 300 px across (0.82 of the page's), the stage 0.3 of the
    // page's cover round it each side.
    let size = (300.0f32 / 0.82 * 1.6).round();
    let stage = Stage { hub: 150.0, ..Stage::new(size, size, (10.0, 4.0)) };
    let look = Preset::Saturn.look(&ThemeViz::default());
    let palette = [[0x8a, 0x4f, 0xff], [0x2c, 0xd5, 0xe0], [0xff, 0x3d, 0xa5]];
    let stops = base_stops(&look, palette[0], &palette);
    let g = halo(&stage);
    let (_, bands) = layout(&look, &stage);
    let dt = 1.0f32 / 60.0;
    let b = brushes(&look, &stage, stage.h, 0.0, &stops, look.strength(true));
    let json = format!(
        "{{\"w\":{},\"h\":{},\"cx\":{},\"cy\":{},\"hub\":{},\"reach\":{},\"line_width\":{},\"glow_width\":{},\"bloom\":{},\"line\":{},\"glow\":{},\"sparks\":{},\"sparks_old\":{},\"sparks_glow\":{}}}",
        stage.w,
        stage.h,
        g.cx,
        g.cy,
        g.hub,
        g.hub + g.gap + g.travel + 1.0,
        b.line_width,
        b.glow_width,
        brush_json(&b.bloom),
        brush_json(&b.line),
        brush_json(&b.glow),
        brush_json(&b.sparks),
        brush_json(&b.sparks_old),
        brush_json(&b.sparks_glow)
    );
    std::fs::write(out.join("saturn.json"), json).unwrap();
    for (name, bpm) in [("120", 120.0f32), ("170", 170.0), ("slow", 72.0), ("quiet", 120.0)] {
        let period = 60.0 / bpm;
        let mut d = Draw::default();
        let frames = (12.0 / dt).round() as usize;
        let mut kick = 0.0f32;
        let mut rows: Vec<Vec<u8>> = Vec::new();
        let mut most = 0;
        for f in 0..frames {
            let t = (f + 1) as f32 * dt;
            let phase = (t / period).fract();
            let beat = phase < dt / period;
            let n = (t / period).floor() as usize;
            let quiet = name == "quiet" && (4.0..8.0).contains(&t);
            let strength = if name == "slow" || quiet { 0.0 } else { [1.0, 0.75, 0.9, 0.75][n % 4] };
            // The slow one's beats: the low notes' onset alone, a little harder some beats.
            let low = if name == "slow" && beat { 3.5 + 0.5 * ((n * 7 % 5) as f32) / 4.0 } else if beat && strength > 0.0 { 4.0 } else { 0.0 };
            if beat {
                kick = kick.max(if strength > 0.0 { strength } else if name == "slow" { 0.45 } else { 0.0 });
            }
            kick *= (-dt / 0.12).exp();
            let gain = if quiet { 0.55 } else { 1.0 };
            let levels: Vec<f32> = (0..bands)
                .map(|i| {
                    let x = i as f32 / bands as f32;
                    let lows = if x < 0.15 { 0.35 + 0.55 * kick } else { 0.0 };
                    let mids = 0.28 + 0.22 * (x * 9.0 + t * 0.7).sin().abs() * (1.0 - x * 0.5) + 0.08 * (t * 3.1 + x * 20.0).sin();
                    (lows.max(mids) * gain).clamp(0.0, 1.0)
                })
                .collect();
            d.tick(dt);
            d.level(if quiet { -12.0 } else { 0.0 });
            d.pulse(Pulse { hit: if beat { strength } else { 0.0 }, period, phase, beat, low, ..Pulse::default() });
            d.draw(&look, &stage, true, &levels, &levels);
            let (p, st) = (&d.paths, &d.saturn);
            let (back, front, glow) = (dot_list(&p.sparks_old), dot_list(&p.front), dot_list(&p.front_glow));
            // Which grains are drawn: the fullness drops them in a fixed order.
            let mut by: Vec<usize> = (0..st.grains.len()).collect();
            by.sort_by(|&a, &b| (a as f32 * 0.618_034).fract().total_cmp(&(b as f32 * 0.618_034).fract()));
            let mut shown = vec![false; st.grains.len()];
            by.iter().take(back.len() + front.len()).for_each(|&i| shown[i] = true);
            let (mut bi, mut fi, mut gi) = (0, 0, 0);
            let mut row = vec![u8::from(beat), (p.pulse.clamp(0.0, 1.0) * 255.0).round() as u8, (p.punch.clamp(0.0, 1.0) * 255.0).round() as u8, (phase * 255.0).round() as u8];
            row.resize(4 + st.grains.len() * 6, 0);
            for (i, q) in st.grains.iter().enumerate() {
                if !shown[i] {
                    continue;
                }
                let (v, layer) = if q.behind {
                    bi += 1;
                    (back.get(bi - 1), 1)
                } else {
                    fi += 1;
                    let v = front.get(fi - 1);
                    let lit = v.zip(glow.get(gi)).is_some_and(|(v, w)| (w.0 - v.0).abs() < 0.11 && (w.1 - v.1).abs() < 0.11);
                    gi += usize::from(lit);
                    (v, if lit { 3 } else { 2 })
                };
                let Some(&(x, y, r)) = v else { continue };
                let o = 4 + i * 6;
                row[o..o + 2].copy_from_slice(&((x * 4.0).round().clamp(0.0, 65535.0) as u16).to_le_bytes());
                row[o + 2..o + 4].copy_from_slice(&((y * 4.0).round().clamp(0.0, 65535.0) as u16).to_le_bytes());
                row[o + 4] = (r * 20.0).round().clamp(0.0, 255.0) as u8;
                row[o + 5] = layer;
            }
            assert!(bi == back.len() && fi == front.len() && gi == glow.len(), "{name} frame {f}: grains not told apart ({bi}/{}, {fi}/{}, {gi}/{})", back.len(), front.len(), glow.len());
            most = most.max(st.grains.len());
            rows.push(row);
        }
        let mut bin: Vec<u8> = Vec::new();
        bin.extend_from_slice(&(frames as u32).to_le_bytes());
        bin.extend_from_slice(&(most as u32).to_le_bytes());
        for mut r in rows {
            r.resize(4 + most * 6, 0);
            bin.extend(r);
        }
        std::fs::write(out.join(format!("{tag}-{name}.bin")), bin).unwrap();
    }
}
