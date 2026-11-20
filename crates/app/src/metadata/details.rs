//! Settings > Window > Show technical details, and the file details of "View metadata".
//!
//! With the setting on (off by default): the player bar shows the playing track's details
//! short ("FLAC 24/96"), now playing in full ("FLAC · 24-bit / 96 kHz · 2,915 kbps") and,
//! under it, what reaches the device and the ReplayGain adjustment ("Output: 96 kHz,
//! exclusive, 24-bit · ReplayGain −6.2 dB, track"), which Settings > Output shows too. With
//! the equalizer changing the sound it says so ("· Equalizer: Bass boost"), so the line
//! never reads as untouched when it is not.
//! Track lists get a quiet column (`rows.rs`), album pages a summary line (`views.rs`).
//! With it off, nothing of it is drawn, nothing is formatted, and the views are as before.
//!
//! The playing track's details are the library's (`tracks.codec` and its neighbours, read
//! by the scan; for a file from outside the library, read when it was opened), with what
//! the decoder found in the file where it says more or otherwise: the sample rate it
//! decodes at, the channels, and a lossless stream's bit depth (`player_core::StreamInfo`,
//! asked for once when a track starts: nothing here runs on a timer).
//!
//! A track's file details ("File" in "View metadata", whether or not the setting is on)
//! are `property_rows` (metadata/view.rs reads the file for them, off the UI thread).

use std::cell::Cell;
use std::rc::Rc;

use library::Track;
use library::audio::{self, AudioInfo, Codec};
use player_core::replaygain::{Applied, GainSource};
use player_core::{OutputInfo, StreamInfo};
use slint::ComponentHandle;

use crate::state::State;
use crate::{Output, Player};

thread_local! {
    static SHOWN: Cell<bool> = const { Cell::new(false) };
    /// The token of the track the engine said it started last (`Event::TrackStarted`
    /// comes before its status names the track).
    static STARTED: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Whether the setting is on (the track models ask on every row they make).
pub fn shown() -> bool {
    SHOWN.with(Cell::get)
}

/// The setting as saved (at start, and whenever the player settings are shown again).
pub fn sync(st: &Rc<State>) {
    let on = st.settings.borrow().show_details;
    if SHOWN.with(|s| s.replace(on)) != on {
        // Rows are made with or without their details: make them again.
        for l in st.lists.all() {
            l.details_changed();
        }
    }
    if let Some(app) = st.app.upgrade() {
        app.global::<Player>().set_show_details(on);
    }
    now_playing(st);
}

pub fn toggle(st: &Rc<State>) {
    {
        let mut s = st.settings.borrow_mut();
        s.show_details = !s.show_details;
    }
    st.save_settings_soon();
    sync(st);
}

/// The playing track's details as shown: the library's, with what the decoder found.
pub fn merged(library: AudioInfo, stream: Option<&StreamInfo>) -> AudioInfo {
    let Some(s) = stream else { return library };
    let mut a = library;
    if !a.is_known() {
        a.codec = s.codec.map(Codec::from_name).unwrap_or_default();
    }
    if s.sample_rate > 0 {
        a.sample_rate = Some(s.sample_rate);
    }
    if let Some(c) = s.channels.and_then(|c| u8::try_from(c).ok()) {
        a.channels = Some(c);
    }
    if a.codec.lossless()
        && let Some(b) = s.bits_per_sample.and_then(|b| u8::try_from(b).ok())
    {
        a.bit_depth = Some(b);
    }
    a
}

/// What reaches the device, so it is plain whether playback is untouched: "Output: 96 kHz,
/// exclusive, 24-bit", "Output: shared, 44.1 kHz, not resampled", "Output: shared, 48 kHz
/// (Windows resamples from 96 kHz)". `source` is the playing track's details.
pub fn output_line(out: &OutputInfo, source: &AudioInfo) -> String {
    if out.rate == 0 {
        return String::new();
    }
    let rate = audio::khz(out.rate);
    // "not resampled" by the system; while the engine converts the track (a crossfade across
    // rates, `conversion_text`) that would read as untouched, so it is left out.
    let untouched = if converted(out).is_some() { "" } else { ", not resampled" };
    // Through a sound server (Linux): the rate its graph runs at, once known, and whether
    // it resamples; "bit-perfect" when the setting asked for the track's rate and got it.
    if let Some(server) = out.server {
        return match out.mix_rate {
            Some(mix) if mix != out.rate => format!("Output: {server}, {} kHz ({server} resamples from {rate} kHz)", audio::khz(mix)),
            Some(_) if out.exclusive.is_some() => format!("Output: {server}, {rate} kHz, bit-perfect rate{untouched}"),
            Some(_) => format!("Output: {server}, {rate} kHz{untouched}"),
            None => format!("Output: {server}, {rate} kHz"),
        };
    }
    if let Some(format) = &out.format {
        let mut t = format!("Output: {rate} kHz, exclusive, {format}");
        // 16-bit devices get a deeper source reduced, with TPDF dither (exclusive.rs).
        if format == "16-bit" && source.bit_depth.is_some_and(|b| b > 16) {
            t.push_str(", dithered");
        }
        return t;
    }
    match out.mix_rate {
        Some(mix) if mix != out.rate => format!("Output: shared, {} kHz (Windows resamples from {rate} kHz)", audio::khz(mix)),
        Some(_) => format!("Output: shared, {rate} kHz{untouched}"),
        None => format!("Output: shared, {rate} kHz"),
    }
}

/// The rate the playing track is converted from, when the engine converts it.
fn converted(out: &OutputInfo) -> Option<u32> {
    out.converted_from.filter(|&r| r != out.rate && out.rate > 0)
}

/// "Converted from 44.1 kHz since the crossfade (a pause returns to 44.1 kHz)": the track came
/// in through a crossfade from a track at the rate playing and is converted to it (the
/// device cannot change rate in the middle of the music); "" when it plays at its own rate.
pub fn conversion_text(out: &OutputInfo) -> String {
    match converted(out) {
        Some(r) => {
            let k = audio::khz(r);
            format!("Converted from {k} kHz since the crossfade (a pause returns to {k} kHz)")
        }
        None => String::new(),
    }
}

/// The player bar's short details while converted: "FLAC 16/44.1 → 48".
pub fn short_with_conversion(short: String, out: Option<&OutputInfo>) -> String {
    match out.and_then(|o| converted(o).map(|_| o.rate)) {
        Some(to) if !short.is_empty() => format!("{short} \u{2192} {}", audio::khz(to)),
        _ => short,
    }
}

/// "Equalizer: Bass boost" while the equalizer changes the sound; "" otherwise (off, or
/// on and flat: then the samples pass through untouched).
pub fn eq_text(changes_sound: bool, preset: &str) -> String {
    if changes_sound { format!("Equalizer: {preset}") } else { String::new() }
}

/// "ReplayGain −6.2 dB, track", "ReplayGain +1.5 dB, album (lowered to its peak)";
/// "No ReplayGain tags" with ReplayGain on and nothing tagged; "" with it off.
pub fn gain_text(applied: Option<Applied>, on: bool) -> String {
    let Some(a) = applied else {
        return if on { "No ReplayGain tags".to_owned() } else { String::new() };
    };
    let db = (a.db * 10.0).round() / 10.0;
    let sign = if db < 0.0 { "\u{2212}" } else if db > 0.0 { "+" } else { "" };
    let source = match a.source {
        GainSource::Track => "track",
        GainSource::Album => "album",
    };
    let mut t = format!("ReplayGain {sign}{:.1} dB, {source}", db.abs());
    if a.capped {
        t.push_str(" (lowered to its peak)");
    }
    t
}

/// The engine started the track with this token (`playback::tick`).
pub fn track_started(token: u64) {
    STARTED.with(|s| s.set(Some(token)));
}

/// Playback ended or stopped: nothing reaches the device.
pub fn ended() {
    STARTED.with(|s| s.set(None));
}

/// The engine's facts on the playing track, if the track playing is the queue's current
/// one (the engine reports it from `TrackStarted` on, until playback ends).
fn current_stream(st: &State) -> Option<StreamInfo> {
    let token = STARTED.with(Cell::get)?;
    let uid = st.tokens.borrow().get(&token).copied()?;
    if st.queue.borrow().current().map(|i| i.uid) != Some(uid) {
        return None;
    }
    st.player.stream_info(token)
}

/// Fill the player bar's and now playing's details, and Settings > Output's line, for
/// what plays now. Cleared with the setting off.
pub fn now_playing(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let p = app.global::<Player>();
    let o = app.global::<Output>();
    let current = st.queue.borrow().current().map(|i| (i.track.audio, i.track.is_remote()));
    let remote = current.is_some_and(|(_, r)| r);
    let (Some(library), true) = (current.map(|(a, _)| a), shown()) else {
        p.set_details_short("".into());
        p.set_details_full("".into());
        p.set_details_spoken("".into());
        p.set_details_output("".into());
        o.set_stream("".into());
        return;
    };
    let stream = current_stream(st);
    let info = merged(library, stream.as_ref());
    // The engine's conversion applies to this track only once it plays it.
    let out = stream.as_ref().and(crate::output::info());
    p.set_details_short(short_with_conversion(audio::compact_channels(&info), out.as_ref()).into());
    // A server track's details are the library's (from the catalog) and the decoder's, as a
    // file's are; where it comes from is said quietly after them.
    p.set_details_full(if remote { format!("{} \u{b7} from the server", audio::full(&info)) } else { audio::full(&info) }.into());
    let conversion = out.as_ref().map(conversion_text).unwrap_or_default();
    let spoken = audio::spoken(&info);
    // "..., converted from 44.1 kHz since the crossfade (...)": its first letter lowered.
    let said = conversion.get(..1).map(|c| format!("{}{}", c.to_lowercase(), &conversion[1..])).unwrap_or_default();
    p.set_details_spoken(if said.is_empty() { spoken } else { format!("{spoken}, {said}") }.into());
    // What reaches the device: only once the engine plays this track.
    let mut parts = Vec::new();
    if let (Some(s), Some(out)) = (&stream, out) {
        let line = output_line(&out, &info);
        let eq = {
            let set = st.settings.borrow();
            eq_text(crate::eq::changes_sound(&set), &crate::eq::summary(&set))
        };
        let shown: Vec<&str> = [line.as_str(), conversion.as_str(), eq.as_str()].into_iter().filter(|t| !t.is_empty()).collect();
        o.set_stream(if line.is_empty() { String::new() } else { shown.join(" · ") }.into());
        if !line.is_empty() {
            parts.push(line);
        }
        if !conversion.is_empty() {
            parts.push(conversion);
        }
        if !eq.is_empty() {
            parts.push(eq);
        }
        let cfg = st.settings.borrow().gain_config();
        let g = gain_text(player_core::replaygain::applied(&s.gain_tags, &cfg), cfg.mode != player_core::GainMode::Off);
        if !g.is_empty() {
            parts.push(g);
        }
    } else {
        o.set_stream("".into());
    }
    p.set_details_output(parts.join(" · ").into());
}

// ---- File details (View metadata) -----------------------------------------------------------------------

/// "34.2 MB (35,860,123 bytes)": sizes as Explorer gives them (1 MB = 1,048,576 bytes).
pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    let short = if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", b / (1024.0 * 1024.0 * 1024.0))
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", (b / 1024.0).ceil())
    };
    format!("{short} ({} bytes)", audio::thousands(bytes))
}

/// The date a Unix time falls on, in local time: "15 Nov 2026".
pub fn date_text(unix_secs: i64) -> String {
    let (y, m, d) = local_date(unix_secs);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{d} {} {y}", MONTHS[(m.clamp(1, 12) - 1) as usize])
}

/// (year, month, day) of days since 1970-01-01 (proleptic Gregorian; Hinnant's civil_from_days).
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The local date of a Unix time (the system's time zone rules, daylight saving included:
/// `platform::system::local_date`), else the UTC date.
fn local_date(unix_secs: i64) -> (i64, u32, u32) {
    crate::platform::system::local_date(unix_secs).unwrap_or_else(|| civil(unix_secs.div_euclid(86_400)))
}

/// "2 (stereo)", "1 (mono)", "6 (5.1)".
fn channels_row(c: u8) -> String {
    match c {
        2 => "2 (stereo)".to_owned(),
        n => match audio::channels_text(n) {
            Some(t) if !t.ends_with("channels") => format!("{n} ({t})"),
            _ => n.to_string(),
        },
    }
}

/// What the File rows know, gathered: the track as listed, the file as read now,
/// its size, and its history (library tracks).
#[derive(Clone, Debug, Default)]
pub struct Known {
    pub report: Option<library::tags::FileReport>,
    pub size: Option<u64>,
    pub extras: Option<library::query::TrackExtras>,
    /// The engine's facts, when this is the track playing.
    pub stream: Option<StreamInfo>,
}

/// The File rows (label, value), in order; rows with nothing to say are left out.
pub fn property_rows(track: &Track, k: &Known) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    // The file as it is now, else as last scanned; the decoder's word where it plays.
    let base = k.report.as_ref().map(|r| r.audio).filter(AudioInfo::is_known).unwrap_or(track.audio);
    let a = merged(base, k.stream.as_ref());
    let mut row = |l: &str, v: String| rows.push((l.to_owned(), v));
    if track.is_remote() {
        // No file here: what the rows say is the server's catalog's, and the decoder's.
        row("Source", "From the server".to_owned());
    }
    if a.is_known() {
        row("Format", format!("{} ({})", a.codec.name(), if a.codec.lossless() { "lossless" } else { "lossy" }));
    } else {
        row("Format", "Not known".to_owned());
    }
    if let Some(b) = a.bit_depth {
        row("Bit depth", format!("{b}-bit"));
    }
    if let Some(r) = a.sample_rate {
        row("Sample rate", format!("{} kHz", audio::khz(r)));
    }
    if let Some(b) = a.bitrate {
        row("Bitrate", if a.codec.lossless() || a.codec == Codec::Unknown { audio::kbps_text(b) } else { format!("{} (average)", audio::kbps_text(b)) });
    }
    if let Some(c) = a.channels {
        row("Channels", channels_row(c));
    }
    let duration = k.report.as_ref().map(|r| r.duration).filter(|d| *d > 0.0).unwrap_or(track.duration);
    row("Duration", crate::rows::format_time(duration));
    if let Some(s) = k.size.or(k.extras.as_ref().map(|e| e.size.max(0) as u64)) {
        row("Size", size_text(s));
    }
    match k.report.as_ref().map(|r| r.replaygain.as_slice()) {
        Some([]) => row("ReplayGain", "No tags".to_owned()),
        Some(tags) => row("ReplayGain", tags.iter().map(|(l, v)| format!("{l} {v}")).collect::<Vec<_>>().join("\n")),
        None => {}
    }
    match &k.extras {
        Some(e) => {
            row("Added", date_text(e.added_at));
            row(
                "Plays",
                match (e.plays, e.last_played) {
                    (0, _) => "Never played".to_owned(),
                    (n, Some(last)) => format!("{} \u{b7} last {}", if n == 1 { "1 play".to_owned() } else { format!("{} plays", audio::thousands(n as u64)) }, date_text(last)),
                    (n, None) => format!("{n} plays"),
                },
            );
        }
        None => row("Library", "Not in the library (opened from outside)".to_owned()),
    }
    rows
}

/// The engine's facts on a track, when it is the one playing (the File rows of "View
/// metadata" take the decoder's word on what it decodes).
pub fn playing_stream(st: &Rc<State>, track: &Track) -> Option<StreamInfo> {
    (st.queue.borrow().current().map(|i| i.track.id) == Some(track.id)).then(|| current_stream(st)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flac24() -> AudioInfo {
        AudioInfo { codec: Codec::Flac, sample_rate: Some(96_000), bit_depth: Some(24), bitrate: Some(2_915), channels: Some(2) }
    }

    #[test]
    fn the_decoder_has_the_last_word_on_what_it_decodes() {
        let lib = AudioInfo { sample_rate: Some(44_100), ..flac24() };
        let s = StreamInfo { codec: Some("FLAC"), sample_rate: 96_000, bits_per_sample: Some(24), channels: Some(2), ..Default::default() };
        assert_eq!(merged(lib, Some(&s)), flac24());
        assert_eq!(merged(lib, None), lib, "not playing: the library's");
        // A lossy stream's decoder depth says nothing about the file.
        let mp3 = AudioInfo { codec: Codec::Mp3, sample_rate: Some(44_100), bit_depth: None, bitrate: Some(320), channels: Some(2) };
        let s = StreamInfo { codec: Some("MP3"), sample_rate: 44_100, bits_per_sample: Some(16), channels: Some(1), ..Default::default() };
        assert_eq!(merged(mp3, Some(&s)), AudioInfo { channels: Some(1), ..mp3 });
        // Unknown to the library (not read yet): the decoder names it.
        let s = StreamInfo { codec: Some("Opus"), sample_rate: 48_000, channels: Some(2), ..Default::default() };
        assert_eq!(merged(AudioInfo::default(), Some(&s)).codec, Codec::Opus);
    }

    #[test]
    fn the_equalizer_is_named_when_it_changes_the_sound() {
        assert_eq!(super::eq_text(true, "Bass boost"), "Equalizer: Bass boost");
        assert_eq!(super::eq_text(false, "Bass boost"), "", "off or flat: untouched, nothing to say");
    }

    #[test]
    fn the_output_line_says_whether_playback_is_untouched() {
        let out = |rate, mix, format: Option<&str>| OutputInfo { rate, mix_rate: mix, format: format.map(str::to_owned), ..Default::default() };
        assert_eq!(output_line(&out(96_000, None, Some("24-bit")), &flac24()), "Output: 96 kHz, exclusive, 24-bit");
        assert_eq!(output_line(&out(96_000, None, Some("16-bit")), &flac24()), "Output: 96 kHz, exclusive, 16-bit, dithered");
        assert_eq!(output_line(&out(44_100, None, Some("32-bit float")), &flac24()), "Output: 44.1 kHz, exclusive, 32-bit float");
        assert_eq!(output_line(&out(96_000, Some(48_000), None), &flac24()), "Output: shared, 48 kHz (Windows resamples from 96 kHz)");
        // Through PipeWire (Linux): the graph's rate, once known; the bit-perfect request.
        let pw = |rate, graph, granted: bool| OutputInfo {
            rate,
            mix_rate: graph,
            server: Some("PipeWire"),
            exclusive: granted.then(|| "at the track's own 96 kHz".to_owned()),
            ..Default::default()
        };
        assert_eq!(output_line(&pw(96_000, None, false), &flac24()), "Output: PipeWire, 96 kHz");
        assert_eq!(output_line(&pw(96_000, Some(48_000), false), &flac24()), "Output: PipeWire, 48 kHz (PipeWire resamples from 96 kHz)");
        assert_eq!(output_line(&pw(44_100, Some(44_100), false), &flac24()), "Output: PipeWire, 44.1 kHz, not resampled");
        assert_eq!(output_line(&pw(96_000, Some(96_000), true), &flac24()), "Output: PipeWire, 96 kHz, bit-perfect rate, not resampled");
        assert_eq!(output_line(&out(44_100, Some(44_100), None), &flac24()), "Output: shared, 44.1 kHz, not resampled");
        assert_eq!(output_line(&out(48_000, None, None), &flac24()), "Output: shared, 48 kHz");
        assert_eq!(output_line(&out(0, None, None), &flac24()), "");
    }

    #[test]
    fn a_converted_track_is_never_shown_as_untouched() {
        let conv = |o: OutputInfo| OutputInfo { converted_from: Some(44_100), ..o };
        let shared = OutputInfo { rate: 48_000, mix_rate: Some(48_000), ..Default::default() };
        assert_eq!(output_line(&shared, &flac24()), "Output: shared, 48 kHz, not resampled");
        assert_eq!(output_line(&conv(shared.clone()), &flac24()), "Output: shared, 48 kHz", "Windows does not resample it, the engine does");
        assert_eq!(conversion_text(&conv(shared.clone())), "Converted from 44.1 kHz since the crossfade (a pause returns to 44.1 kHz)");
        assert_eq!(conversion_text(&shared), "");
        let excl = OutputInfo { rate: 48_000, format: Some("24-bit".into()), exclusive: Some("24-bit / 48 kHz".into()), ..Default::default() };
        assert_eq!(output_line(&conv(excl.clone()), &flac24()), "Output: 48 kHz, exclusive, 24-bit");
        assert!(conversion_text(&conv(excl)).starts_with("Converted from 44.1 kHz"));
        let pw = OutputInfo { rate: 48_000, mix_rate: Some(48_000), server: Some("PipeWire"), exclusive: Some("48 kHz".into()), ..Default::default() };
        assert_eq!(output_line(&pw, &flac24()), "Output: PipeWire, 48 kHz, bit-perfect rate, not resampled");
        assert_eq!(output_line(&conv(pw), &flac24()), "Output: PipeWire, 48 kHz, bit-perfect rate");
        assert_eq!(short_with_conversion("FLAC 16/44.1".into(), Some(&conv(shared.clone()))), "FLAC 16/44.1 \u{2192} 48");
        assert_eq!(short_with_conversion("FLAC 16/44.1".into(), Some(&shared)), "FLAC 16/44.1");
        assert_eq!(short_with_conversion("FLAC 16/44.1".into(), None), "FLAC 16/44.1");
    }

    #[test]
    fn the_gain_line() {
        let a = |db, source, capped| Some(Applied { db, source, capped });
        assert_eq!(gain_text(a(-6.2, GainSource::Track, false), true), "ReplayGain \u{2212}6.2 dB, track");
        assert_eq!(gain_text(a(1.46, GainSource::Album, false), true), "ReplayGain +1.5 dB, album");
        assert_eq!(gain_text(a(0.01, GainSource::Track, false), true), "ReplayGain 0.0 dB, track");
        assert_eq!(gain_text(a(1.94, GainSource::Track, true), true), "ReplayGain +1.9 dB, track (lowered to its peak)");
        assert_eq!(gain_text(None, true), "No ReplayGain tags");
        assert_eq!(gain_text(None, false), "");
    }

    #[test]
    fn dates_and_sizes() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_407), (2025, 11, 15));
        assert_eq!(civil(19_782), (2024, 2, 29));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(size_text(35_860_123), "34.2 MB (35,860,123 bytes)");
        assert_eq!(size_text(1_500), "2 KB (1,500 bytes)");
        assert_eq!(size_text(3 * 1024 * 1024 * 1024), "3.0 GB (3,221,225,472 bytes)");
    }

    #[test]
    fn properties_rows() {
        let t = Track { title: "One".into(), duration: 245.0, audio: flac24(), ..Track::default() };
        let k = Known {
            report: Some(library::tags::FileReport { audio: flac24(), duration: 245.2, replaygain: vec![("Track gain", "-6.20 dB".into()), ("Track peak", "0.988".into())] }),
            size: Some(35_860_123),
            extras: Some(library::query::TrackExtras { size: 1, added_at: 0, plays: 0, last_played: None }),
            stream: None,
        };
        let rows = property_rows(&t, &k);
        let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Format", "Bit depth", "Sample rate", "Bitrate", "Channels", "Duration", "Size", "ReplayGain", "Added", "Plays"]);
        assert_eq!(rows[0].1, "FLAC (lossless)");
        assert_eq!(rows[3].1, "2,915 kbps");
        assert_eq!(rows[7].1, "Track gain -6.20 dB\nTrack peak 0.988");
        assert_eq!(rows[9].1, "Never played");
        // A lossy file from outside the library: no bit depth, an average, no history.
        let mp3 = AudioInfo { codec: Codec::Mp3, sample_rate: Some(44_100), bit_depth: None, bitrate: Some(245), channels: Some(1) };
        let t = Track { id: -3, audio: mp3, ..Track::default() };
        let rows = property_rows(&t, &Known { report: None, size: Some(10), extras: None, stream: None });
        let get = |l: &str| rows.iter().find(|(x, _)| x == l).map(|(_, v)| v.clone());
        assert_eq!(get("Bit depth"), None);
        assert_eq!(get("Bitrate").as_deref(), Some("245 kbps (average)"));
        assert_eq!(get("Channels").as_deref(), Some("1 (mono)"));
        assert!(get("Library").is_some() && get("Plays").is_none());
        assert_eq!(get("ReplayGain"), None, "not read: nothing said");
    }
}
