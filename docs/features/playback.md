# Playback

Resuming, ReplayGain, the equalizer, crossfade and the output device.

## Resuming

The queue (with the order shuffle turns back to), the current track and the position in
it, shuffle and repeat, and the window's size, position and maximized state come back on
the next start. Playback comes back paused: nothing plays until play is pressed, and then
it starts where it stopped. Tracks no longer in the library drop out of the queue. The
position is saved every 5 s while playing and at once on a pause, a track change and
exit; the queue when it changes. A window saved on a monitor that is no longer there
opens on the primary one. A track from the server resumes the same way: the engine waits,
loading, for the bytes at the saved position, which are asked for there (a range request; an
MP3 waits until its download gets there: see [Streaming](lyrics.md#streaming)).

## ReplayGain

Track, album or off (player bar, or Settings). Opus files use `R128_*_GAIN` (Q7.8 dB,
relative to the OpusHead output gain, -23 LUFS) plus 5 dB, so they match
`REPLAYGAIN_*` files (-18 LUFS). The Opus header gain always applies. The matching peak
tag (`REPLAYGAIN_TRACK_PEAK` for a track gain, `REPLAYGAIN_ALBUM_PEAK` for an album gain)
caps the gain when present, the other peak when only that one is tagged. Files without tags play unadjusted. Preamp and clipping
protection are in Settings > Playback (Preamp, Clipping protection), and in
`settings.json` (`preamp_db`, `prevent_clipping`).

## Equalizer

Settings > Equalizer (`app/src/eq.rs`, `ui/eq.slint`; the engine's `player_core::eq`), off
by default. The player bar's equalizer button opens it and is lit while it is on; the
technical details name it ("Output: 96 kHz, exclusive, 24-bit · Equalizer: Bass boost"), so
the output line never reads as untouched when it is not.

**What it is.** Up to 32 second-order bands, each a peak, a low or high shelf, a low-pass, a
high-pass, a band-pass (0 dB at its peak), a notch or an all-pass, from the Audio EQ Cookbook
(a shelf's frequency is its midpoint, its Q the cookbook's). A band's width is a Q, or as
Equalizer APO writes them a bandwidth in octaves (the cookbook's digital bandwidth, `sinh` form)
or a shelf's slope S (APO's "x dB per octave" is S = x / 12); the filters without a gain have
none to set. Everyone sees ten sliders (the graphic layout: peaks an octave wide, Q 1.41, at
the ISO centres 31.5 Hz to 16 kHz, ±12 dB) under the curve, each slider under its own octave of
it; **Parametric** shows the bands in full: a band's kind is a button opening the eight kinds
(Enter, the arrows, Enter), and each shows only what it has (a low-pass: frequency and Q; a
peak: frequency, gain and its width in its own unit, "Q 1.41", "1 oct", "10.8 dB/oct"). A
setting the sliders cannot show (other kinds, other frequencies, more bands) is not changed by
looking at it in the graphic view: the sliders show the closest they come (least squares,
`presets::fit_graphic`) and a line under them says so, how far off that is and what moving one
does (it replaces the bands with the ten sliders). The curve is a card of its own under the
section's title, pinned like the visualizer's preview (see [Settings](settings.md#settings)): beside the column on a
wide window, held at the column's top while the sliders or the bands scroll on a narrow one.
It is drawn from the bands' own response at the rate playing (48 kHz when nothing plays): the bands, filled, and a thin line
with the preamp added. It is worked out from each band's analog prototype at the bilinear
transform's warped frequency, which is exactly the digital filter's response (a unit test
checks the two derivations against each other for every kind and width at five rates) and
keeps its precision far below a high-pass's corner.

**Copy and Paste.** **Copy** (beside the preset; `Ctrl+C` on the page, outside a text field,
unless remapped) puts the setting on the clipboard as Equalizer APO text, the form APO, Peace,
AutoEq and Room EQ Wizard read and write, and the pill says "Equalizer copied":

    # Marquee Player equalizer preset: FiiO FT1
    Preamp: -4.3 dB
    Filter 1: ON LSC Fc 105 Hz Gain -2.7 dB Q 0.7
    Filter 2: ON PK Fc 155 Hz Gain -3.4 dB Q 1.47
    Filter 3: ON LPQ Fc 16000 Hz Q 0.5

What it says is what plays: the ten sliders as the peaks they are, the auto preamp as its value
now (with `# Marquee Player: preamp auto`, so this app reads it back as auto), a width in its
own form (`BW Oct 1`, `HSC 9 dB Fc …`), a Butterworth low-pass as plain `LP`; with the
equalizer off, every filter `OFF` and `Preamp: 0 dB` (APO then plays nothing either, as here).
Numbers have at most two decimals for frequencies, so APO's rule reading "1.000" as a thousand
never applies. The chevron beside Copy copies it as a **GraphicEQ** line instead (the curve
that plays at AutoEq's 127 frequencies, for Wavelet) or as an **EasyEffects** preset (its "APO
(DR)" band mode, which is exactly these filters).

**Paste** (`Ctrl+V` on the page unless remapped; a text field keeps its own) reads the
clipboard, and **Import a preset file…** in the preset menu reads a file; neither changes
anything yet. What it holds is shown first, over the page:

- what it is ("Equalizer APO text", "AutoEq fixed band EQ", "Room EQ Wizard filters
  (Generic)", "EasyEffects preset", "GraphicEQ curve") and its filters and preamp in one line;
- a verdict at a glance: "Exact: plays as written", "2 things left out (below)", "A close match:
  within 0.34 dB of the curve" (lit when something was left out or a fit is rough);
- the curve it would play over the one playing now (and a GraphicEQ's own curve);
- every line left out or played approximately, with why (a per-channel filter, an effect with
  no equivalent, an EasyEffects band of a design with no counterpart), and quieter, what was
  read in a particular way (an OFF filter, a corner frequency moved to the midpoint);
- every filter (kind, frequency, gain, width), and the preset it becomes ("Adds the preset
  “FiiO FT1” and plays it"; a preset that is this setting already is chosen, not added twice).

**Apply** (Enter: it has the keyboard) adds it as a preset named for it (the file's or the
text's name, AutoEq's "… ParametricEQ" taken off; "Pasted equalizer" otherwise) and plays it;
**Cancel**, Esc or a click outside leaves the equalizer exactly as it was. While it is open, Tab
goes round its buttons and the arrows and Page Up/Down scroll it; no shortcut fires. Text that
is no equalizer setting at all says so in the pill ("Nothing to paste: no equalizer settings on
the clipboard"); one that is recognised but cannot be applied (APO's `IIR` coefficients, an
`Include:`) shows the preview with the reason and Close. The reading runs on a thread of its
own (a GraphicEQ fit takes about a tenth of a second); nothing runs otherwise. Export offers
Equalizer APO text or this app's own preset file (below).

**The formats** (`player_core::eq::formats`, `eq/apo.rs`, `eq/fit.rs`), each checked against
its tool's own source or documentation (read 2026-11) and tested on real files; the content
decides what a text is, a file's name only names it:

| Format | Read | Left out or refused, said |
| --- | --- | --- |
| Equalizer APO configuration (also Peace's export) | As Equalizer APO reads it: `Preamp` (summed); `Filter` lines of every type, `PK` `PEQ` `Modal` `LP` `LPQ` `HP` `HPQ` `BP` `LS` `HS` `LSC` `HSC` `NO` `AP`, with `Q`, `BW Oct` or a shelf's `x dB` slope; APO's defaults (LP, HP, BP Q 0.707; NO Q 30; a shelf with neither Q nor slope S 0.9); corner-frequency shelves (`LS 6dB`, `LS 12dB`, `LS … Q`) moved to their midpoint by APO's own factor; every comma a decimal point; "1.000 Hz" as 1000 Hz (APO's thousands rule); `OFF` filters and REW's `None` slots play nothing; `GraphicEQ` lines (fitted, below); `Channel: all`, `L R`; `Device: all`; `Stage: post-mix`; keywords in any letter case (said: APO reads only its own) | A filter missing what it needs (no Q on a peak, no frequency: APO plays nothing either), an unknown type (REW's `LP1`, `HP1`, `LT`), a non-numeric preamp: left out, the line named. Filters and preamps for one channel, for channels a stereo output does not have, or under `Device:` patterns: left out, said (this equalizer treats both channels alike, on every device). `Copy:`, `Delay:`, `LoudnessCorrection:`: left out, said. Refused, with the line: `IIR` coefficients (for one sample rate), `Include:`, `Convolution:`, `If:` / `Eval:` and inline expressions |
| AutoEq `ParametricEQ.txt`, `FixedBandEQ.txt` | APO text (as AutoEq writes it: `PK`, `LSC`, `HSC`, integer Fc, Q to 0.01; fixed band: peaks at 31.25 · 2^n Hz, written 31, 62, 125…, Q 1.41), named from the file | As APO text. A fixed band file is ten peaks at 31 and 62 Hz, not the sliders' 31.5 and 63: shown as bands |
| GraphicEQ (AutoEq's `GraphicEQ.txt`, Wavelet, APO's `GraphicEQ:`) | The curve as APO plays it (linear over log frequency, flat beyond its ends; several lines add up), fitted with bands, its offset in the preamp | Approximated, always said, with the fit's largest and RMS deviation. Refused beyond 3 dB (below). At most 10,000 points |
| Room EQ Wizard "Export filter settings as text" | Its header lines ignored, its filters as APO reads them: the Generic equaliser's `Q` form and the FBQ2496's `BW Oct` form (APO's reference names both), the equaliser named | The DSP1124P's `BW/60` lines: left out, said (export with the Generic equaliser) |
| EasyEffects presets (7, 8: `output["equalizer#0"]`; 6: `output.equalizer`) and PulseEffects 4 presets (every value a string) | "APO (DR)" bands: the cookbook filters as APO's, exact (Bell and Resonance peaks; shelves; Lo-/Hi-pass, Notch, Allpass, Bandpass; Ladder-pass and -rej as two shelves at the band's edges). "RLC (BT)" bands: the cookbook's prototypes through the same transform with LSP's own Q, exact: a Bell at slope xN is N peaks of gain/N, Q `sqrt(g)/(k sin(atan g))`; shelves N of gain/N, Q (1+Q)/2; Lo-/Hi-pass N second-order sections; Notch; Allpass N+1. Input plus output gain is the preamp; pitch shifts every frequency; muted and Off bands play nothing; solo plays only the solo bands | "RLC (MT)" played as its BT form, said. BWC and LRX bands, RLC Resonance, Bandpass and Ladders: left out, said. FIR/FFT modes (linear-phase): played minimum-phase, said. Balance: left out, said. Other effects in the preset: said. Refused: left and right set differently (split channels, or a pitch per side), every equalizer bypassed |
| This app's own preset file | JSON, `"marquee_player_equalizer": 2` its version: name, preamp (`"auto"` or dB), bands (`type`, `freq`, `gain`, and `q`, `octaves` or `slope_db`), every number as it is. An older build's export (APO text with the `# Marquee Player equalizer preset:` comment, version 1) still reads, name and auto preamp included | A newer version's file reads as far as this one knows it (said); a kind of band it does not have refuses, naming it |

Not read, and why: **miniDSP** biquad coefficient lists (recognised and refused: coefficients for
one sample rate; REW's Generic export of the same filters reads exactly); **CamillaDSP**
configurations (recognised and refused: a YAML pipeline, not a filter list); **Peace**'s own
`.peace` configurations (which also hold per-speaker setups and effects; their layout is not
documented, and was not checked against Peace's published source, so they are not guessed at;
Peace's Equalizer APO text reads). Every format is read only from text the user gives: nothing is fetched.

Bounds, whatever is pasted: at most 1 MB of text (refused before any parsing beyond; on Windows the
clipboard's size is checked before it is copied out); text with NUL or many control characters is
"not text"; files in UTF-8, UTF-16 with a byte order mark (Notepad's "Unicode") or a single-byte
code page; at most 32 filters, more refused with the count ("40 filters: the equalizer takes up
to 32", never cut short); numbers held within the equalizer's ranges (said); at most 60 notes
listed, the rest counted.

**GraphicEQ.** A GraphicEQ line is a curve, not filters, and Equalizer APO plays it through a
16,384-tap minimum-phase FIR filter; this engine has no FIR, so the curve is fitted with bands
(`fit.rs`): peaks placed one at a time where the curve is furthest from what is there (a shelf
where that is at an end), each refined with all the others by Levenberg-Marquardt least squares
on the dB curve every 1/24 octave from 20 Hz to 20 kHz, at 44.1, 48 and 96 kHz at once (near
Nyquist a band bends differently at each rate: fitted at 48 kHz alone, AutoEq's FiiO FT1 curve
missed by 3 dB at 19.9 kHz played at 44.1), with a light penalty on large gains so bands do not
fight; adding stops at 0.15 dB, or when four more bands gained under 0.03 dB; bands under
0.15 dB are dropped. The result is measured with the engine's own response, worst of the three
rates, every 1/96 octave: within 1 dB it applies as any import (the preview says how close);
up to 3 dB the preview says it is a rough match; beyond 3 dB it is refused. Bands are
minimum-phase like APO's FIR, so a close magnitude is a close phase. AutoEq's FiiO FT1
GraphicEQ (127 points): 25 bands, within 0.34 dB (RMS 0.06 dB), in 0.13 s.

**Where it runs, and in what order.** Per write on the output thread (`output.rs`): each
track's gain (ReplayGain with its preamp and clipping protection, the Opus header gain),
then the equalizer preamp and bands, then the peak limiter, the visualizer's tap, the volume
and the fades. The order matters: ReplayGain first, so the equalizer sees a level-matched
signal and its preamp means the same on every track; the limiter after the equalizer, which
can put back peaks the limiter would have taken (a boost, a cut's or an all-pass's phase shift
raising a peak: measured below), so it sees what plays; the volume last (it only lowers).
Not on the decode thread, which runs a second ahead: a slider acts within the device buffer
(~40 ms; measured 70 ms of audio to within 0.5 dB of a +12 dB jump, glide included). With
the equalizer on, the decode thread leaves limiting to the output thread (`Chunk::limited`);
with it off, the decode thread limits as before and the output thread touches nothing. The
output thread never waits: it takes a new setting with `try_lock` (the handle's lock is held
for a copy of a fixed-size setting), computes coefficients and the auto preamp without
allocating, and works through a fixed 32-frame buffer; what it does per callback is bounded by
the 32 bands, whatever was pasted.

**Numerics.** f64 direct form I with each delay's feedforward and feedback terms paired
(`b1 x1 - a1 y1`), so a band at 0 dB passes samples through bit for bit and a coefficient
change acts on the signal's own history; f64 keeps the poles exact at 20 Hz and 384 kHz
(`1 - cos w0` is worked out as `2 sin² (w0/2)`, without the cancellation at low frequencies);
denormals are flushed. Coefficients are worked out per sample rate (a band above 45% of the
rate is held there). A change glides: gain, log frequency and log width follow a one-pole of
20 ms, recomputed every 32 frames, the preamp interpolated per sample. A filter without a gain
glides by its mix: `(1 - m) + m H(z)`, the same poles with the numerator `(1 - m) a + m b`, the
identity at m = 0 bit for bit (as a 0 dB peak is) and H at m = 1; so a low-pass comes in, a
notch goes out, without a step. A band changing kind (or how its width is given) goes through
the identity. Turned off, everything glides to the identity, then it bypasses
(`Equalizer::is_bypassed`).

**Headroom.** The preamp is Auto by default: down by the combined curve's largest boost
(`eq::max_boost_db`, searched every 1/48 octave and refined), so no frequency comes out
louder than it went in; a curve that only cuts gets 0 dB. A resonant low- or high-pass (Q above
0.707) boosts at its corner and gets the headroom for it (Q 8: −18 dB); a Butterworth one, a
notch, the band-pass and an all-pass never boost and get none. ReplayGain's preamp and clipping
protection apply before it, as always. Manual sets it by hand. The limiter (Settings > Playback
> Limit loud peaks) catches what is left: a sweep's transient through a boosted resonance (1–2%
over), the summed peaks of noise, a square wave whose peaks an all-pass or a low-pass raises
(+65% through the all-pass below: the level of every frequency unchanged, the waveform's peaks
not).

**Presets.** Flat, Bass boost and reducer (a ±6 dB low shelf at 105 Hz, the Harman bass
shelf), Treble boost and reducer (±4 dB high shelf from 6 kHz), Vocal (+3 dB at 2.5 kHz, −3 dB
below 150 Hz), Loudness (a rise below 1 kHz to about +9 dB at 31 Hz, +5 at 125 Hz, +1 at 500 Hz,
the treble left alone: the general shape of the ear losing bass first as the level drops; rounded
numbers, not fitted to the ISO 226 tables, which are not freely published and were not checked),
Warm and Bright (a ±0.45 dB/octave tilt through 700 Hz), Spoken word (Vocal,
less rumble and hiss). Each is the ten sliders fitted to that target curve (`eq/presets.rs`;
a test checks the fit and that each stays within 1 dB of its target, the treble shelves
within 1.6 dB). The user's presets (pasted and imported ones too) are saved, renamed and deleted
from the preset button's menu, and exported as Equalizer APO text or as this app's own file.

**Settings and files.** `settings.json` keeps each band as before (`kind`, `freq`, `gain`, `q`)
with the new kinds (`low_pass`, `high_pass`, `band_pass`, `notch`, `all_pass`) and a `width`
(`octaves`, `slope`) only where it is not a Q, so a file from before reads and writes back
unchanged. (A build from before this change reading a file with the new kinds loses the
`equalizer` entry to its defaults, and says so in its log: it cannot know them.)

**Per device.** With a device playing (or chosen, or the system default once listed),
"Separate setting for <device>" keeps a setting for that device alone, by its id
(`settings.json` `equalizer.devices`); whenever it plays, its setting does, and the other
devices share one. Following the system default, the setting follows the device Windows
plays on.

**Off is bit-perfect.** Off, the output is bit-identical to before (the existing sample-exact
tests pass unchanged; `tests/eq.rs` checks it with boosts set and the equalizer off, and on
with every band at 0 dB). A peak or shelf with a Q has exactly the coefficients it had before
the other kinds existed, and its processing is the same arithmetic, so unchanged presets play
bit-identically too. On, gapless boundaries stay sample-continuous (the filters carry
on: the engine's output equals the equalizer run over the pieces joined, bit for bit); seeks
and play-now start the filters afresh, a pause keeps them, a device switch at the same rate
keeps them, a rate change (a reopen between tracks) starts them afresh.

Measured (`cargo test -p player-core --release --test eq -- --nocapture`, the development PC):

| | |
| --- | --- |
| Response through the engine vs the filters' curve, 21 tones 20 Hz–20 kHz (shelves, peaks, cuts, boosts, Q 0.7–3) | worst 0.0003 dB at 44.1 kHz, 0.0002 at 48, 0.0001 at 88.2 and 96, 0.00006 at 192, 0.00003 at 384 kHz |
| The same for every other kind and width (high-pass, slope shelves, notch, all-pass, a peak by octaves, low-pass, band-pass by octaves), against the analytic curve | worst 0.0007 dB at 44.1 and 48 kHz, 0.0003 at 96, 0.0002 at 192, 0.00006 at 384 kHz; the engine bit-identical to the filters run offline |
| An all-pass at 1 kHz, Q 0.7 | every level within 0.00001 dB; its phase within 0.001° of the prototype's (−180° at its centre) |
| A low-pass coming in while playing, then becoming a notch | no sample step larger than the steady signal's |
| A +12 dB slider jump / a drag in 0.5 dB steps | largest sample-to-sample step no larger than the steady signal's at +12 dB; within 0.5 dB of the new level 70 ms of audio after the call |
| Seek and pause with Loudness on (40 Hz + 2 kHz) | no step larger than the steady signal's (+0.5%); a pause no bump at all, a seek's bass settles in with +0.5 dB at most for a few tens of ms |
| Every built-in preset, a full-scale sweep and full-scale noise, auto preamp | out of the engine at most 0.9999 (the ceiling); the preamp alone keeps the sweep within 1.2% of full scale |
| A resonant low-pass (Q 8) and high-pass (Q 10), a Butterworth low-pass, a notch with an all-pass; a full-scale sweep and square wave | auto preamp −18.1, −20.0, 0 and 0 dB; equalized peaks 0.998, 0.927, 1.10 and 1.68; out of the engine at most 0.9999 |
| CPU per second of audio | 10 bands: 1.2 ms at 44.1 kHz, 2.7 at 96, 5.4 at 192; 16 bands: 2.0, 4.3, 8.6 ms; 32 bands (peaks, or every kind): 3.9, 8.6, 16.9 ms (1.7% of a core at 192 kHz, the worst case on the output thread) |
| Taking a new setting on the output thread (coefficients and the auto preamp's search), 32 bands at 192 kHz | 0.036 ms, once per change |
| AutoEq's FiiO FT1 files | ParametricEQ and FixedBandEQ: their filters exactly, copied and read back the same; GraphicEQ: 25 bands within 0.34 dB, RMS 0.06 dB, through the equalizer within that of APO's curve |

The limiter's snap back to unity used to skip frames needing under 0.009 dB of reduction, so
samples up to 1.0009 went through; it now limits those too (the change touches only samples
that were over its ceiling).

## Crossfade

Settings > Playback > Between tracks (`app/src/crossfade.rs` decides; the engine mixes), off
by default: Crossfade, its length (1–12 s, 6 by default), Keep albums gapless (on), Crossfade
when skipping (on).

**The rule.** A track that ends goes into the next with an equal-power crossfade (`cos`/`sin`
over the overlap: level for unrelated material, which adds in power; no curve choice), each
track at its own ReplayGain, the sum through the equalizer and the limiter as one signal. The
overlap is at most a fifth of either track (so play counts and scrobbles still come by their
own rules) and none under half a second, so a track under 2.5 s is gapless. It stays gapless
when the next track is the one after it on the same album in the album's order (disc,
number, file name, as the album page lists it), with Keep albums gapless on: live albums,
DJ mixes split into tracks, concept albums and movements are albums played in order, which
is what this catches. Two tracks of one album out of order (shuffle, a playlist) crossfade
like any others. Files opened from outside the library count as an album by folder, album
tag and consecutive numbers. Repeat one is gapless (a loop of the track itself). A queue that
ends, a next track that cannot be opened, or an outgoing track of unknown length: no
crossfade, the track plays out. An audio test of "continuity" (a track starting loud) was
considered and left out: too many tracks start on a downbeat.

**Skips.** Next, previous, a track chosen in a list or the queue, while playing, crossfade for
half a second (at most the length set) from the very sample being heard: the output thread
keeps what was playing, from the chunks already decoded (`out_chunks`), and mixes it under the
new track's fade-in. It is no longer than what is decoded of the old track (about a second at
44.1 and 48 kHz, a quarter at 192). A new track at another rate is converted to the rate
playing (below), so it crossfades the same way. A skip that lands just as one decoded chunk
has been written out to its end crossfades too, from the next chunk queued (it used to fall
back to the short cut: the output thread took and dropped that chunk before it saw the skip).
Paused, stopped, or a second skip during a skip's crossfade: as before (the short cut).
Pause, seek and stop keep their fades; "previous" past 3 s still restarts the track (a seek).

**In the engine.** `Player::set_next_with(next, Transition::Crossfade { secs })`. The decode
thread, within reach of the end, waits for the next track's open if needed, works out the
overlap from both lengths and decodes the current track exactly to its start (`Playing::read`
keeps a packet's remainder), then decodes both and mixes them into one chunk stream; the
chunks before the middle of the overlap carry the outgoing track's token and position, from
the middle the incoming one's. So the **current track changes in the middle**, when the
incoming one becomes the louder: `TrackStarted` (now playing, the player bar, media keys,
the tray, lyrics, Last.fm's now playing, the session's saved position) switches there, the
outgoing track's position runs to its length minus half the overlap, the incoming one's starts
at half the overlap. A seek before the middle seeks the outgoing track (the incoming one waits
again from its start, and the crossfade comes round again); after it, the incoming one, which
then plays alone. Play counting and scrobbling measure each track against its own length from
what was shown (a unit test runs 3 s to 20 min tracks with 12 s crossfades both sides: all
counted and scrobbled).

**Sample rates.** A crossfade between tracks at different rates (an Opus track, always 48 kHz,
into a FLAC at 44.1, the commonest case in a mixed library) overlaps like any other. The device
runs at one rate, so one of the two tracks has to be converted during the overlap, and the
choice is which:

- *The incoming track, to the rate playing* (what the engine does). The device is never
  reopened in the middle of the music. The price: the incoming track is still converted when
  the overlap ends, and going to its own rate means reopening the device, which is a gap. So
  it **stays converted until the stream is cut anyway**: a pause (the output asks the decode
  thread for the frame after the pause at the track's own rate, `Cmd::Native`, and the resume
  reopens the device at it, with the resume's usual 5 ms fade-in), a seek, a skip without
  crossfade, a stop, the end of the queue. A later track at the rate playing plays as it is
  (crossfading or gapless); a gapless track at a third rate reopens the device, as before; one
  at the converted track's own rate (the rest of the album the crossfade went into) goes on
  through the same converter, so the album stays gapless, its boundaries as continuous as at
  its own rate. After a pause, the track is bit for bit what it is played alone.
- *The outgoing track's tail, to the incoming rate*, reopening the device before the overlap so
  the incoming track plays at its own rate throughout. Rejected: the reopen would be a gap in
  the middle of the outgoing track's music (tens of milliseconds in shared mode, and in
  exclusive mode many DACs mute for longer while they relock to a new rate), which is exactly
  the discontinuity a crossfade is meant to remove. Only a silent moment would hide it, and a
  crossfade has none.

In every mode the device format sequence is the same: no reopen through the crossfade (shared,
exclusive at its format, PipeWire's bit-perfect graph at the outgoing track's rate), one reopen
at the incoming track's own rate when the stream is next cut. Nothing else is ever converted:
with crossfade off, and for every track not reached through a crossfade across rates, the
samples are bit-identical to before (the sample-exact tests pass unchanged). The technical
details say when it happens: `Output: 48 kHz, exclusive, 24-bit · Converted from 44.1 kHz since
the crossfade (a pause returns to 44.1 kHz)`, the player bar `FLAC 16/44.1 → 48`, and "not
resampled" is left out of the line while it is so (`OutputInfo::converted_from`, reported when
it changes). Exclusive mode mixes in f32 before the format conversion, as for same-rate tracks.
A pair of rates with no converter (an odd rate: below) keeps the old fallback, the outgoing
track fading out over the crossfade's length, the device reopening, the next starting as it is.

**The converter** (`player_core::resample`) is written here, not taken from a crate: an exact
rational polyphase windowed sinc. The ratio is reduced (48,000 / 44,100 = 160 / 147) and output
frame `n` is the band-limited value of the input at time `n M / L`, from `L` phases of 224
coefficients (at 44.1 to 48 kHz; more, in input frames, going down), each output from its own
phase's coefficients, summed in f64, rounded once to f32. `rubato` (MIT, the usual pure-Rust
choice) was considered: its arbitrary-ratio sinc resampler interpolates between table entries,
an error floor of its own, and its exact fixed-ratio mode is FFT-based, with block latency and
the `rustfft` family in the build; neither gives frame-exact counts and alignment, or a
converter carried across a gapless boundary, without wrapping. Written here it is about 300
lines, no dependency and no third-party notice; the exe grew by 24,064 bytes with it, the
engine's use of it and the app's wording (44,250,112 to 44,274,176, release profile, no bench).

- Passband to 0.4535 of the lower rate (20.0 kHz at 44.1, 21.8 at 48), stopband from the lower
  rate's Nyquist frequency (nothing aliases, not even into the transition band), Kaiser window
  designed for 155 dB. Opus is itself band-limited to 20 kHz.
- Linear phase: symmetric impulse response, a constant delay that is taken out entirely
  (output frame 0 aligned with input frame 0), so the overlap stays sample-aligned with its
  envelope and positions stay exact. Minimum phase would delay each frequency differently and
  no single shift could align the two tracks; the pre-ringing it would avoid is at the
  transition band (above 20 kHz at 44.1), lasts 2.2 ms to -120 dB, and is excited only by
  content up there.
- Primed with silence (a track starts from silence) and flushed with it: `N` input frames
  give exactly `ceil(N L / M)` output frames, the first aligned with the first input frame.
- Streaming: input pushed in blocks, output pulled frame by frame, the result the same bit
  for bit however it is split; no allocation once running (a counting allocator checks it).
  Memory: the table (287 kB at 44.1 to 48 kHz, 1.2 MB at 44.1 to 192, at most 8 MB) and a
  buffer of a few thousand frames. The table is built in 1 to 6 ms (21 at 44.1 to 768 kHz) and
  kept for the next crossfade between the same rates.
- Equal rates are a bypass, bit for bit. A pair whose table would be over 2^20 coefficients
  has no filter: none of the usual rates (8 to 768 kHz in both families), but say 44,101 Hz.
- Latency: 112 input frames read ahead at 44.1 to 48 kHz (2.5 ms, about the same at any
  pair), absorbed by the decode thread's lead; there is no delay in the output.

Measured (`cargo test -p player-core --release --test resample -- --nocapture`, every pair of
44.1, 48, 88.2, 96, 176.4 and 192 kHz both ways, levels by DFT at whole hertz over one second):

| | |
| --- | --- |
| Passband ripple, 11 tones from 0.1% to 99.9% of the passband edge | at most 0.00000016 dB |
| Images and aliases of passband tones (the first 50 images) | at most -164.6 dB |
| Tones above the lower Nyquist going down (would alias) | at most -152.9 dB |
| THD+N, full-scale 1 kHz | -150.5 dB worst (-158.2 best); the f32 input tone itself measures -153.8; 24-bit's floor is -146.3, 16-bit's -98 |
| Impulse response | symmetric to f32 precision, peak on the impulse's own output frame; ringing to -120 dB 2.2 ms each side at 44.1, 1.1 at 88.2/96, 0.5 at 176.4/192 |
| Round trips (44.1-48-44.1, 44.1-192-44.1, 96-44.1-96...) | the same length back; error -160 to -170 dB rms, -141 dB at worst on any sample |
| Intersample overs | full-scale samples whose true peak is +3 dB come out at 1.4142 (+3.01 dB): the limiter takes them |
| Lengths | exact for 0, 1, 2, 3, M-1, M, M+1, L, 1000, a second and a second plus one, every pair and 384/768 kHz |
| CPU, as the decode thread does it (blocks of 1024 in, chunks of 4096 out) | 2.3 to 2.8 ms per second of audio at 44.1-48 either way; 9.2 to 11.8 at 192 kHz either way; 21 at 44.1-384, 42 at 44.1-768; the worst single chunk 0.85 ms (4.8% of what it plays for) |
| CPU through the engine (decode thread, a 44.1 kHz track crossfaded into at 48) | 0.85 ms more per second of audio than the same at 48 kHz |
| Crossfade off: the engine as fast as it goes, 66 s of six tracks gapless, best of 7, three runs each | 80.4, 80.8, 80.9 ms before the change; 81.0, 80.9, 80.1 after: no difference |

It runs on the decode thread, which is a second ahead (a quarter of a second at 192 kHz, still
far more than the worst chunk); the output thread, real-time, converts nothing: it mixes a
manual skip's crossfade from chunks that come converted already (`Shared::soft_rate` tells the
decode thread the rate it would mix at). The same converter would allow an "always output at a
fixed rate" option later: `resample::Filter` and `Resampler` take any pair, and
`Playing::convert` is the one place the decode thread decides to use one; the option would set
it for every track instead of the incoming one of a crossfade. Not built.

Found on the way, and fixed: when the queue ended, the output let the device buffer play out
and then sent `Ended` without looking whether a track had been started meanwhile; the app,
which moves on at `Ended`, then skipped the track just started (the `skip-damaged` bench on
Linux hit it about one run in five, its second play landing in PipeWire's 45 ms; `tests/ended.rs`).
That `Ended` is now dropped when a newer play, seek or stop has come.

With "Limit loud peaks" off (Settings > Playback), the intersample overs of a converted track go
out as they are, as a hot master's do; on (the default), the limiter after the conversion keeps
them under full scale.

Tested against ffmpeg-made tones, a frequency per track (`tests/crossfade.rs`, sample for
sample): the mix's envelope with each track at its own ReplayGain; the middle switch; an album
in order bit-exact gapless with crossfade on, then the next album crossfading; crossfade off
bit-identical; short tracks (gapless, or a fifth of them); the fade-out and reopen for a rate
with no converter; the queue ending and a next track that fails; seeks before and after the
middle; a pause and a skip in a crossfade; a manual skip's crossfade from the frame being heard
(the capture device holding still while it is asked for, so where it lands does not depend on
the machine's load); repeat one; two loud tracks summing over full scale, limited (with the
equalizer on too). Across rates (`tests/crossfade_rates.rs`, the expected stream made with the
converter run over the incoming track alone): 48 into 44.1, 44.1 into 48, 192 into 44.1 and
44.1 into 192 kHz to the sample, both tones in the middle of the overlap at -3 dB each and
nothing at the pitch an unconverted track would have, no step at either edge; the middle switch
with positions in the track's own seconds; a fifth of a short incoming track; an album gapless
through the converter as one signal, then a 48 kHz track as it is; a gapless track at a third
rate, reopened and untouched; repeat one; a next track that fails; seeks before and after the
middle (after it, at the track's own rate); a pause in the overlap losing nothing and one after
it resuming at the track's own rate, bit for bit after its fade-in; a skip in the overlap; a
manual skip's crossfade both ways; intersample overs limited; the device formats in exclusive
mode and PipeWire's bit-perfect mode (fake devices: 48 kHz through the crossfade, 44.1 from the
resume after a pause; the conversion reported, then not).

## Output device and exclusive mode

Settings > Output > Output device lists the active render devices (enumerating opens
nothing). "System default" (the default choice) follows Windows' default, also when it
changes while playing. A chosen device is remembered by its id (`output_device` in
`settings.json`, with its name to show while it is away). If it is not connected at start,
or goes away while playing, the system default plays meanwhile and the pill and Settings
say so; when it comes back (a device notification, nothing is polled), playback returns to
it. A switch while playing lets what is in the device buffer play out and continues on the
new device from the same sample (`player_core::output`, `tests/output_switch.rs`).

On Linux the device list is PipeWire's sinks and exclusive mode is "Bit-perfect playback":
see [Linux](../linux.md#linux). Exclusive mode (Settings > Output, off by default) opens the device without the Windows
mixer at the track's own sample rate, in the best format it accepts: 32-bit float, then
32-bit, 24-in-32, 24 and 16-bit integer (`output/exclusive.rs`). Volume and ReplayGain still
apply, in software, and the equalizer while it is on; reducing to 16-bit adds TPDF dither, except to a 16-bit track nothing has changed (volume at 100%, no ReplayGain, equalizer, limiting or fade), which reaches a 16-bit device bit for bit. Tracks of the same rate stay
gapless; a rate change reopens the device, as in shared mode, except into a track a crossfade
brings in at another rate, which is converted to the device's format until the next pause (see
[Crossfade](#crossfade)). Paused for 3 s, or stopped,
the device is let go so other applications get their sound back. When the device accepts
no format, does not allow exclusive mode, or another application holds it, playback falls
back to shared mode and Settings and the pill say why. The negotiation, fallback and sample
conversion are unit-tested against a fake device; the WASAPI exclusive stream itself
(polled, `PollingExclusive`) has not been run on hardware yet.
