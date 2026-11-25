# Visualizer

Now playing shows the music moving (`visualizer/mod.rs` runs it, `visualizer/draw.rs` draws it,
`visualizer/look.rs` is its look and its settings; the engine's tap and the analysis are
`player_core::spectrum`). By default it is the look it always had, Classic: faint bars in the
playing cover's accent along the bottom of the page, behind the cover and the panel, with peak
caps and their mirrored reflection. Behind the page the bars rise from the player bar's top
edge (as the bonfire's coals do) and the reflection lies under the bar, seen only through its
frosted glass, drawn stronger by what the glass's tint holds back (`1 / (1 - its alpha)`, about
4 times: visualizer/mod.rs `brush`) so it shows there as the dimmer copy its strength says (with
the glass off the bar is opaque and hides it); in the panel, on the full page
and in the Settings preview it sits under the bars in the stage. Settings > Visualizer, or `V`, turns it on and off (on by default).

**Styles** (`visualizer::look::Style`), each drawn by a few `Path`s rebuilt per frame:

| Style | What | Drawn as |
| --- | --- | --- |
| Bars | Rounded bars, caps above, a scaled mirror below. **With the beat** (below; not with Smooth motion, so not Calm nor the default where Windows' animations are off): the low bars take the kick the moment it lands (the lowest fifth of the bands, the lowest lifted most, by 0.35 of the height at a full kick, less further up and none past that fifth, on top of the spectrum's own level and shaped like any level, so nothing pins to the top; mostly gone in an eighth of a second), every peak cap is kicked up a little with it (a lifted bar carries its cap) and falls as before. The whole field and its reflection brightening on the beat (their alpha 1.3 times at a full one, settled before the next, over 0.22 of a beat) is off: watched on real music on 2026-11-25 the flash was too jarring and at times came with nothing much heard; `MP_BARS_WITH=flash` turns it on (docs/testing.md). Mirrored, the middle bars take it. At rest, with no beat, a frame is exactly as it was | one path of rectangles (and one for the caps, one for the reflection), on whole pixels without anti-aliasing; the brightening a number (`Visualizer.fill-light`) the page lays on the brushes, not a brush rebuilt |
| Line | A smooth curve (Catmull-Rom as cubic Béziers) over a filled area, faint at its foot; its reflection a mirrored area brighter toward its edge. **With the beat**, as the bars (not with Smooth motion: Calm stays still): its low end takes the kick (the lowest fifth of the curve), and the line thickens from 2 px to 4 px at a full beat, settled before the next; its area and the reflection are fainter when the music is quiet against its loudest lately (as `Loud` has it for Saturn: at 12 dB under, its alpha 0.55 of what it is when loud), so a drop looks fuller than a breakdown. It has no glow to widen (a second wide stroke cost more than it showed) | an area path and a 2 px stroke (thickened by a number, `Visualizer.line-swell`) |
| Ring, as Ripples | The cover as a drum skin (`visualizer::draw::radial`). The spectrum is a glowing contour around it: the low notes (the lowest eighth of the bands) swell the whole ring at once, the rest of the spectrum shapes it, low mids at the top to highs at the bottom, the two halves mirrored (no seam where 20 Hz meets 20 kHz: the lows are not on the ring at all, the highs meet themselves); smoothed around, moving as fast as bars do (slowed down, as it once was, it hardly moved). **Intensity** (50% to 200%, this kind only) is how far it swings for the same music, bent toward its reach rather than clipped at it. Between the cover and the contour, its corona: light at the cover's edge thinning outward; behind the cover a bloom that brightens with the low notes. On each beat (the low notes, and a little of everything, rising above where they have been: told when the rise tops out, `Beat`) the contour as it is then leaves the cover as a **ripple**: out a whole cover's radius, its shape kept half again as pronounced, lower and softer the farther it is, turned a little round as it goes (0.34 rad over its way), through the look's colours and fading out (bright over the first half of its way, then quickly fainter: thrown ripples spend most of their time far out). **Every kick has its ripple** (none nearer the last than 0.42 of a beat): as many at once as there is room for (one per 36 px of the way out, three to five), and when one more leaves the oldest, out at the edge where it is all but gone, hurries out of its way (it used to hold the newcomer back: with room for five and a way of five beats the sixth kick found the first one's ripple at the very end of its life, and whether it was gone was a matter of a frame's rounding and of the tempo heard a hair long, so a beat now and then went without, and the timer's ripple after it fell between the beats). **How a ripple moves** is the music's doing, not its age's: the kick that sends it throws it (0.55 of the way for the hardest kick, 0.6 of that for the softest, dying away over 0.3 of the way's time; how hard is the kick's strength as told, raised by how loud the low notes get in the 0.15 s after, a kick being told before its peak), every later kick pushes all those on their way (a tenth of the way for a full one, over a sixth of a second), and between kicks they drift, as fast as it takes for the way to last as many beats as there is room for ripples (with the tempo known; without it, as far apart as the kicks' ripples have lately left; 95 px a second at the slowest): a harder kick's ripple is out fast and slows, a soft one's goes evenly, and the whole field surges on the beat. **Without kicks** (none for a beat and a half: a breakdown, an intro, music that has none) nothing leaves by the clock: a ripple leaves when the music swells (the rise told when it tops out, `Beat`, the larger the rise the harder it is thrown), or after 2.4 s with sound and no swell; where the music keeps a beat without kicks, that ripple waits for the next beat and leaves on it. **The snares' rings are off** (`MP_HALO_WITH=rings` shows them): tried on real music on 2026-11-25, a ring could not be connected to any sound heard as it appeared, while the ripples were liked; they are kept for another try at tuning them. As built: a snare, a clap sends a **ring**: the contour smoothed, as wide as a ripple and nearly white (the look's colours seven tenths of the way to white: told from the ripples at a glance), quickly out and slowing, over the whole of the ripples' way in two beats (it overtakes them), bright until the last third of it; four at once at the most, none nearer the last than 0.42 of a beat. Which sharp edges are a snare's: the tracker's snap is any of them, a kick's own edge and a plucked note's too (in the house fixture every kick was one, in the ballad every plucked note: 67 in 30 s and not a drum in it), so a ring is kept back for two frames and shown only if the sound has stayed bright since (`beat::Heard::bright`, 0.07 or more: a snare's or a clap's noise lasts, a kick's edge is over in milliseconds, a note was never bright). On the fixtures: all 32 claps of the house track (and 3 of its 31 kicks without one), all 22 snares of the hip-hop track and none of its kicks, 34 snares of the rock track, 44 of the build's and all 18 claps of the drop in "Dynamics", none in the ballad or the ambient track. **Where in the beat** (with a beat to go by, `meter::Meter`, below): what counts as a beat depends on it, for this kind and every other that moves with the beat (`draw::beat_hit`, applied once to what is heard each frame: what sends a ripple, the contour's jump, the sparks' burst, the cover's pulse, and what moves Saturn, the eclipse and the bonfire; until 2026-11-25 all but the ripples went with every kick told, a bass line's notes among them). On the beat (within 0.14 of a beat): a kick told (one weaker than 0.55 only if the low notes' onset is at least twice the usual), or an onset of the low notes eight times the usual (a kick the tracker missed under a bass line; never sooner than a twentieth of a beat before the beat, and if the kick is told a moment later the ripple is thrown as hard as that kick). On the half beat: a firm kick (0.75) that the low notes bear out. Off both: nothing. Measured over a minute each of 64 tracks of every kind from a real library (the steady ones: 3,964 beats): of the kicks told, 57% were on the beat, 19% on the half beat and 24% off both, as strong off the beat as on it (a bass line's notes, which fall anywhere), and with every one of them sending a ripple, as they did, the ripples left on the music's beats 49% of the time (chance: 21%) and the tempo read off the kicks was known 40% of the time. With the meter and these rules: 66% on the beat, 83% on the beat or the half beat (chance: 42%), three beats in five with their ripple, the tempo held 89% of the time. A ring too is then a snare's or a clap's on the beat only (an open hat on every half beat is as bright: the rings were 1.8 a second, and are 0.9). While the music has no beat to go by, a hit weaker than 0.85 is no kick to any look unless it is 0.55 or more and the low notes bear it out (three times the usual): the ambient fixture, which has none, was told 92 weak ones in 30 s. **Hits for nothing** (2026-11-25: "pulses for nothing at times" on songs without a clear kick, for the halo and the bars alike): over a minute each of 63 tracks from a real library, half of them without a clear kick (ambient, acoustic, jazz, score, vocal, slow songs), a hit was taken to be for nothing when neither the low band nor the whole sound rose in the 60 ms around it, against the half second before, more than at four moments in five of the same track taken at random (at random 73% of moments are without such a rise). Until then an onset of the low notes three times the usual counted on the beat: it gave three hits in five, and half of them were for nothing (a bass line wavering, a held note's swell); at eight times the usual, 27%. Weak kicks on the beat the low notes did not bear out were 47% for nothing, and kicks of 0.55 to 0.85 with no tempo known, about half. With the rules as they are: 63 hits a minute where there were 98, 17 of them for nothing where there were 42 (on the music with a clear kick 15 where there were 36; on the rest 18 where there were 47), 66% within 50 ms of a reference beat where there were 60% (chance: 20%); of the beats with an audible rise of the low notes, 77% have a hit on music with a clear kick (85% before) and 49% on the rest (62%). What is still left: a quarter of the hits for nothing are on the tracks whose tempo is wrong (14 of the 33 without a clear kick, 3 of the 30 with one: held at two thirds or four thirds of the beat, half a beat off, or none held), where the hits land on the beat no more often than by chance; and the kicks the tracker tells on music without a clear kick are for nothing a third of the time however strong. `MP_HALO_WITHOUT=throw,push,swells` leaves any of these out, for comparing. And **sparks** (`Sparks`): a few leave the contour all the time, where it is loudest, and every beat throws a burst; each flies outward, slowing, and goes out as it ages (it shrinks: one path cannot fade each dot by its age), with a soft disc of light round the young ones; as many as the way out has room for (60 to 240). Peak caps do not apply. A still frame (the preview with nothing playing) shows a few ripples and sparks as a moment would leave them | the contour as one closed curve (stroked twice: glow and line), the corona as that curve with the cover's place left out of it, the ripples as one path of closed curves a third as detailed as the contour, the snare's rings as another (half as detailed again), three paths of dots for the sparks (young, old, discs), each with a radial brush that fades with the distance, and a radial-gradient rectangle for the bloom |
| Ring, its other kinds | Everything drawn around the cover is one style, **Ring** (`Style::RING`; Settings' second choice, "Around the cover"), its kinds sharing the round cover, the bloom behind it and a thin contour or rim. **Saturn** (`saturn`): a tilted ring of dust in orbit (its short axis 0.3 of its long one, turned 18 degrees), the grains on its far side drawn under the cover and those on the near side over it (`VisualizerFront`), each as big as its note is loud. The cover is drawn as a heavy body the dust is bound to, and a beat as the cover pulling, never pushing (since 2026-11-25: the owner found the ring a little boring, its 3D depth the best of it). **Kepler's speeds**: the ring reaches near the cover (1.1 to 1.88 of its radius) and every grain goes at the speed its distance gives it (the distance to the power -1.5, none at a pace of its own), so the inner edge goes round 2.3 times as fast as the outer and laps it, the grains crossing the cover's face outrunning those at the ends. With a tempo it keeps time: a grain halfway out goes a sixteenth of a turn a beat, half of it in a surge right after the beat and the rest coasting to the next (a faster song turns faster; without a tempo it goes at its own pace, hurried by the bass and the beats); and each beat sends a band of light once round the ring, the grains it passes drawn as if their notes were louder, gone within the beat. **The pull**: each beat draws the ring in toward the cover, up to 16% of its distance at a full beat (more nearer the cover, never nearer than 1.04 of its radius), most of the way within a twentieth of a second, turning faster as it comes in (its turn over the square of how far in it is drawn, as a skater's spin), and it eases back out over the beat (over 0.35 of a beat; without a tempo 0.18 s); the bass does not swell it (behind the page the median grain goes 8% in on a full beat at 120 a minute). **Infall**: grains leave the ring's inner quarter, more with the bass (up to three a second) and more on a beat (two or three for a full one), and spiral down in the ring's plane, faster and faster and brighter as they fall (behind the cover on the far side, over its face on the near side), gone at the cover's edge in under a second; each is fed back at the outer edge, brightening in over 0.7 s, and drifts back in to its place; with neither bass nor beat nothing falls, and no more than a tenth of the grains fall at once, so it stays a ring. Tried on 2026-11-25 and dropped: each beat knocking dust out of the ring to settle back (it read as an explosion), two moons on tilted ellipses of their own (they read as moons rather than as the cover's pull), a spoke of light lit on each beat to show the shear (it read as a bright patch standing still) and gaps in the ring (they did not show). Nothing goes further out than the ring's outer edge. A still frame (the Settings preview) draws the ring whole and calm, every grain in its place. A frame costs about 0.12 ms at a 300 px cover. `saturn_trace` (docs/testing.md) writes its frames over made-up music for an animation to judge it by. The louder the music against its loudest lately, the denser and brighter the ring (below). **Eclipse** (`eclipse`): a glow at the rim, wider with the bass, and on each beat a flare of sparks leaving the rim a fifth of a turn (0.19) further round than the last, leaning one way and the next the other, pulled back into the cover in an arc, so the flares circle the cover in time (they left where the music was loudest, which jumped about); a hard beat (over 0.8) throws a pair, the second opposite the first. Without beats: with a tempo, a small flare when the music swells (the rise told when it tops out, `Beat`, as the halo's ripples have it), no more than one a beat, none by the clock; without a tempo, a small one after 1.5 s with sound. All of them fade to nothing in silence. **How loud** (`draw::Loud`, Saturn): the auto level (below) makes a quiet verse fill the display as a loud chorus does, so how far it has slid its scale (`Analyzer::level_slide_db`) is the music's own level; eased over a second and a half, against the loudest it has lately been (followed up at once, forgotten at a third of a dB a second), 12 dB under that is as quiet as it gets (40% of its grains). **Pulse the cover** (on by default; `Look::cover_pulse`): the ring's cover is drawn 2.5% larger at the top of a beat (`Paths::punch`, an eighth of a second), whatever its kind. **Taken out** on 2026-11-25 (the owner found they looked out of place and silly beside the others): **Galaxy**, two spiral arms of dust seen from above, and **Swarm**, dust in a whirl blown out by each beat. A look that names one (the current look or one of the user's presets in settings.json, or a preset pasted or imported, `visualizer::share`) shows as Saturn, the nearest of what remains, the rest of it kept; settings.json is written back so at the next save (`Style::Saturn`'s serde aliases, and `preset_id` for the Galaxy and Swarm presets), and a shared one says so in its notes | dots in three paths (bright or young, dim or old, and discs of light), Saturn's near grains in two more over the cover, a thin stroked contour (the eclipse's glow under it widened per frame by a number, `Visualizer.glow-swell`, not a path) |
| Bonfire | The ring's sparks without the circle (`visualizer::draw::bonfire`): the spectrum as a low glowing line along the stage's foot (its coals: low notes on the left, a sixth of the height at most, its area faintly filled), and sparks rising from it where the music is loudest, slowing and swaying a little as they go, each going out as it ages; a few all the time, a burst on every beat; as many as the stage has room for (60 to 420). Behind the page its stage is the whole page (`Visualizer.tall`), not the band along the foot: the sparks rise behind the cover and the panel. Coloured across the page (by frequency) or up it (by height: a spark turns through the colours as it rises). No reflection, no peak caps | an area path and a 2 px stroke for the coals, three paths of dots for the sparks |

**The halo's cover.** The ring is always around a cover (`VisualizerStage.hub`): behind the
page, around the page's own cover, drawn round at 82% of its box so the contour fits the room
the square had; its ripples and sparks go on past it for most of its radius (the stage reaches
three tenths of the cover beyond it), fading out behind the title, which does not move; all of
it under the cover and the text; in the panel's tab and on the full page, around a round copy of the playing cover at its
stage's middle (0.58 of half the stage's smaller side; on the full page the corner thumbnail
gives way to it); in Settings' preview the same. Following the cover's rounded square instead
was tried and looked like a bristly frame (`MP_HALO_SHAPE=square` still draws it, for
comparing). The contour stays inside its stage and reaches out half the cover's radius at
most: a ring, not a blob. The ripples and sparks travel a whole radius: where the stage is a
box of its own (the panel's tab, the Settings preview) they leave it at its edge (it clips);
where it is the page (behind it, and the full page, `VisualizerStage.open`) they go on and fade.
Colours: the Halo preset takes the cover's palette around the ring (Colour by frequency: the
first colour at the top, the last at the bottom, both sides alike); by height, outward. The
corona and the ripples always run outward through the colours: one brush cannot both turn with
the ring and fade with the distance, and the fade is what they are.

Taken out after being offered: **Shockwave** (a kind of ring: each beat a whole ring of
sparks) and **Fireworks** (a burst of sparks somewhere on the page on each beat), with the
Fireworks preset. A settings file, a preset of the user's or a shared line that names one
reads as its nearest relative (`#[serde(alias)]` on `Style`; `vizshare`): Shockwave as
Ripples, Fireworks as the bonfire.

Left out: "lumi" and outline bars and stereo layouts (parameter variants of
bars rather than looks of their own). A mirrored layout (low notes in the middle) is an option
of Bars and Line rather than a style. Taken out again after being built: LED bars and the
waveform (an oscilloscope, with `spectrum::Scope` behind it) and their presets Retro and Glow,
and a Horizon (one line with pulses running along it). A settings file or a shared preset that
names one of them is not read as that style: the settings fall back to Classic, and such a
preset of the user's is kept in the file untouched and not listed.

**Colours** (`Colours`): Accent (the cover's colour, fainter at the foot: Classic), Cover (a
gradient of up to three of the cover's own colours, `library::accent::palette`: the same hue
vote as the accent, read further down, each made light and vivid enough for the dark theme; a
one-colour cover gets its hue turned a little each way), Classic (green, yellow, red, a meter's
colours), Prism, Rainbow, Ember, Ice, Sunset, and Custom (three of sixteen named colours).
Colour by height (the gradient runs up the bars; for the halo, out from the cover) or by
frequency (across; around the halo). All brushes are built in Rust when the look, the stage or
the colours change, not per frame. The cover's palette is worked out off the UI thread when a
track starts, only while a look uses it (a 360 px thumbnail decoded and voted, a few ms).

**The beat** (`visualizer/beat.rs`, `beat::Tracker`): the ring and the bonfire move with the kicks, not
with a rise in the spectrum they draw (a 170 ms window, smoothed again: a kick in it is a slow
swell, told late), and since 2026-11-25 so do bars and the line, which until then had a kick
only through that window, soft and late (their rows above say how; with Smooth motion they
do not listen for it at all, `Look::hears_beat`). The tracker listens to the newest 35 ms of the sound itself each frame.
*Kicks*: the energy at five frequencies from 45 to 110 Hz (Goertzel under a Hann window, no
state between frames); a kick is that energy rising sharply, told the moment the rise passes
what a kick needs (a frame or two after the kick starts), no weaker than 42% of recent kicks'
rises, within 7 dB of the loudest the band has lately been, and, for 0.3 s after a kick, only
if it is as loud as that one (a kick's tail swelling back against a bass note is not a second
kick). *Snaps*: the same for the sound's sharp end (the energy of its sample-to-sample
change): a snare, a clap. *The tempo and the beats* (`visualizer/meter.rs`, `meter::Meter`) are not read off the kicks: on
real music a bass line's notes are told as kicks and a kick under a rolling bass is missed.
The meter hears everything that starts, as beat trackers do (it follows Stark, Davies and
Plumbley's BTrack most closely). Every 10 ms the spectrum of the newest 21 ms in sixteen
bands from 35 Hz to 11.5 kHz, each band's level compressed; the onset function is how much
the bands rose since the hop before, and the low notes' share of it apart. The tempo: the
onset function's last six seconds correlated with themselves, read by a comb (a beat's length
is a lag at which it repeats, and at twice, three and four times that lag), tempos near 140
a minute preferred, broadly; the scores are held and eased and the tempo kept while it scores
within a fifth of the best; a newly chosen tempo is checked against the ones at two thirds
and four thirds of its beat and at twice it, by laying a train of pulses at each spacing over
the onsets (the comb cannot always tell a beat from one and a half). Since 2026-11-25 the
newest three seconds are read alone too, and when they are sure of another tempo (not the one
held, twice it or half it) two readings running, the held one is let go and the tempo read
afresh from those three seconds: the six seconds hold the music before a change for as long as
it fills most of them, and a tempo that had stopped fitting was held five to eight seconds
(124 a minute over an 88 track), where the looks, moving only with what falls on the beat,
barely moved; now two to three. On made-up music at fifteen steady tempos and three frame
rates it never lets go (asking the held tempo to score 0.9 of the best instead of 0.75, it
let go of one). The beats: a running
score (each hop's onset plus the best score of about a beat ago), run on into the future half
a beat after each beat to find the next: a beat is told when it happens, not after. Whether
there is a beat at all: how far the best tempo's score stands above the others' (noise, a film
score, a piano played freely repeat a little at some lag as anything does: nothing is told
for them). Whether the beat is on the meter's pulse or halfway between two is left to the
kicks when they say it plainly (`beat::Count`: an open hat on every half beat can outweigh
the kicks in what the meter hears). Against a trained beat tracker run offline (beat_this) on
those 64 minutes: the beats told within 70 ms of the reference's, or of its half beats or
every other one, for 79% of them, 42 tracks over 80%; slow songs are mostly told at twice
their tempo (their eighth notes are as steady as their beats), and three tracks in free time
have no beat told. A beat without its kick is no longer made up from the tempo: the meter's
beats are there, kick or no kick (`Heard::beat`). What the looks do with it
(`visualizer::draw::Pulse`): only a kick where the music's beat is counts (on the beat, or a
firm one on the half beat; without a tempo, a sure one: the halo's row above says how), and so
does an onset of the low notes on the beat with no kick told, as a soft one. One beat is one
hit (`Draw::one_hit`): an onset stays above what counts for two or three frames and the kick
itself may be told a frame after it, so a hit sooner than 0.42 of a beat after the last one
(between 0.16 and 0.3 s; without a tempo, 0.16 s) is dropped; until 2026-11-25 only the
ripples had such a gap. Each is a jump of
the whole ring (dying away in an eighth of a second), a ripple, a burst of sparks, the cover's
pulse and what moves the ring's other kinds and the bonfire, larger for a harder kick (Saturn's
pull and band of light, the eclipse's flare); with the tempo known, Saturn's
ring also turns with the beat, by where in the beat the frame is
(`Pulse::phase`); a snap
is a smaller burst and, round the halo, a thin ring; every such kick also pushes the ripples on their way, and with the
tempo known their way out takes as many beats as there is room for ripples, so those on their
way are about a beat apart and move in time (the halo's row above says how).
Unit tests on made-up tracks (a kick on every beat from 92 to 174 a minute at 30, 60 and 144
frames a second, each kick heard once and within 70 ms, the tempo within 1.5% and each beat
told within 45 ms of a kick; a hip-hop bar with its second kick half a beat late; a missing
kick; a bass note between kicks; a pad, noise and silence, which have no beat; a frame a
quarter of a second long), and
`viz-look` on the fixtures: 123.8 heard for the house track's 124 with 17 kicks told for 17
beats played in 8 s, 176.0 for the hip-hop track's 88 (twice it: its eighth notes).

**Where it shows** (`Placement`): behind the page (a band along the bottom; the bars at most a
quarter of the page tall, `[visualizer] height` at most, their baseline at the player bar's top
edge and their reflection under the bar, its share of the bars' height but no taller than the
bar: `VisualizerStage.refl-room`, `Stage::refl_room`, `draw::split`), as a Visualizer tab of Now playing's
panel beside Up next and Lyrics (`Y` gives way to the lyrics), or the full page (the cover and
title in the top left corner, the stage everywhere else). `Shift+V` (remappable: "Where the
visualizer shows") moves it to the next (`visualizer::look::cycle_placement`).

**Opacity** (Settings > Visualizer > Opacity, and the quick menu's Opacity ▸, 10% to 100%):
how strongly it is drawn where it shows. Behind the page and elsewhere (the panel, the full
page) each keep their own (`Look::prominence`, saved under its old name "prominence", and
`Look::opacity`): a look behind the page is faint so the page reads over it, and the same look
in the panel is the content. The control edits the one where it shows now; its line says the
other. It replaces "Strength behind the page", whose value is the behind-the-page opacity as it
was: nobody's look changed (the panel and full page, never adjustable before, start at 100%,
as they were drawn). Every built-in preset keeps its own behind the page and 100% elsewhere.

**Presets** (`Preset`), each a whole look (not the placement): Classic (as before), Prism (the
bold one: a bright line in a soft spectrum over its reflection, at 85% behind the page; see
below),
Skyline (fine mirrored bars over a clear mirror), Calm (a slow, soft line in the accent, for
a quiet room or less motion), Bonfire (sparks rising from a glowing line, in the cover's
colours, 80% behind the page), Halo (the ring, as Ripples, in the cover's colours, 85% behind
the page), and one for each other look drawn around the cover, the halo's look with its own
drawing: Saturn, Eclipse. In that order everywhere they are listed. Changing
anything makes the look Custom (or the preset it then equals). `C` (remappable: "Next
visualizer look") shows the next look, the built-in presets then the user's in their order,
and says its name in the pill. Reset goes back to Classic, behind the page; the user's presets
stay.

**The user's presets** (`visualizer/presets.rs`, the Presets button's menu in Settings > Visualizer):
Save as preset… (the name prompt; a built-in name is refused, a taken one offers Replace),
Rename… and Delete for the one in use, Update to this look once it has changed since it was
chosen (`VizSettings::based_on`), Copy this look as text, Paste a preset…, Export as a file…,
Import a file…. They show after the built-in ones in Settings' picker (a row of their own, a
small mark on each), in the quick menu (Your presets ▸) and in `C`'s round. A preset is the look
only: not where it shows, not whether it is on. Choosing one stores its look in full
(`preset: "custom"`), so an older build shows it as it is. Saved in settings.json as
`"visualizer_presets"` (`visualizer::look::UserPresets`), apart from `"visualizer_look"`, read entry by
entry: one this build cannot read (a newer build's style, a damaged one) is kept as it was and
written back so, never costing the others; the file without any is as before.

**Sharing** (`visualizer/share.rs`): one line of text, versioned, every value by name, so it survives a
chat message and can be read:

    MarqueeViz/1 name="Night halo" style=ring colours=cover by=frequency detail=medium motion=normal sensitivity=0 intensity=100% peaks=on mirror=off reflection=off reflection-height=45% reflection-strength=55% opacity=100% behind=85% custom=3c64ff,a050f0,ff6eaa

Copy puts it on the clipboard (Ctrl+C on the page too); a `.mpviz` file holds a comment line
and it. Reading finds the line anywhere in the text (a message around it), takes keys in any
order and case, smart quotes too, a missing key the default look's value; an unknown key is
left out and said, a number out of range brought in and said, a key given twice keeps the
first. Refused, said plainly in the pill and nothing changed: text with no preset ("Nothing to
paste: no visualizer preset on the clipboard"), more than 16 KB (a file over 64 KB is not read),
a NUL or another control character, a newer format ("it is from a newer version of Marquee
Player (format 2); this version reads format 1"), a style, colour scheme, detail or speed this
build does not have, a value that is not a number or a colour, an unclosed quote. A later build
that only adds keys keeps format 1 (this one leaves them out); one that changes a key's meaning
raises it. Pasted or imported, a preset plays in Settings' preview (the look in use stays
under it, the controls still show it) with the question under the preview: Add preset; Replace
or Keep both (as "Name (2)") when one of the user's has that name; Use it when that is the same
preset already; Cancel or Esc. Leaving the section, changing the look or Reset cancels it. A
built-in name is never taken over (the shared one becomes "Halo (2)").

**Now playing's menu**: the button at the top right of the page (or a right click on the page
itself) opens on and off, Opacity ▸ (100% to 10%, where it shows now), then under the heading
**Presets** the ten built-in ones and Your presets ▸ (when there are any), under **Where it
shows** the three placements, and Visualizer settings…, all checked as they are; the keyboard
works in it (the arrows, Right into a submenu, Enter). A look is chosen whole here: what it is
made of (its style, its colours) is Settings' business, so the menu has no Style ▸ (it had,
and nothing said how a style differed from the presets under it). A heading
(`menus::Entry::Heading`) is a small dim caption over the entries it names, 26 px tall, which
the pointer and the arrows pass over. A menu may have several submenus (`menus::open_sub`
fills each when it opens).

**Settings > Visualizer** (`VisualizerSection` in `ui/pages/settings/visualizer.slint`): a live preview (the real
analysis while music plays; with nothing playing, one still frame of made-up music,
`visualizer::still_levels`, drawn once) that stays in view while the settings scroll (see
[Settings](settings.md#settings)), on and off, where, opacity, the presets (one Tab stop, the arrows choose; the
user's after the built-in ones), Your presets (the menu) and Reset, then the style, the colours
(swatches of each gradient, one Tab stop), the custom colours, colour by, detail (fine, medium,
coarse: the bars' size or the curve's points), mirror, peak caps, the reflection (on, its
height and strength), the speed (smooth, normal, snappy: the analyser's attack and decay, the
peaks' hold and fall), the sensitivity (±12 dB in 3 dB steps) and the frame rate. "Find a
setting" knows each. Every control is keyboard reachable with the focus ring; the stage is
decorative (`accessible-role: none`), the preview says in words what it shows.

**Saved** in settings.json as `"visualizer_look"` (`visualizer::look::VizSettings`: `preset`, `look`
in full when Custom, `placement`, `fps`, `based_on` while the look is one of the user's), every field defaulted, unknown values brought into
range, an unknown preset (a newer build's) read as Classic. It is absent until the user chooses
anything; then the default follows Windows' "Show animations in Windows"
(`SPI_GETCLIENTAREAANIMATION`): on, Classic; off, Calm (read once at start;
`MP_REDUCED_MOTION=1` or `0` stands in for it). **Precedence with theme.toml**: Settings choose
the style, colours, reflection, motion and placement; `[visualizer] height` is the band's height
behind the page and `bar_width` / `bar_gap` the bars at Medium detail, for every look; `opacity`,
`reflection`, `reflection_opacity` and `peaks` are Classic's values (so a theme tuned before
keeps Classic as it was tuned, and editing them redraws it). A change in Settings wins: the look
becomes Custom and is saved in full. `reflection_opacity` now means the reflection's alpha where
it meets the bars as a share of theirs (0 to 1); a file still holding the old default 1.6 (which
made up for the glass it hid behind) is moved to 0.55, a tuned value kept (clamped to 1).

**The tap and the analysis.** The engine taps the samples as they are written to the device,
after ReplayGain, the equalizer, a crossfade's mix and the limiter, and before volume and fades
(`player_core::spectrum::Tap`): what is shown is what is heard (with the equalizer's boosts and
cuts), it does not shrink with the volume slider, works in shared and exclusive mode at every
sample rate, and is still while paused. Nothing about the tap changed for the new styles: the
wave reads the same ring (the newest 70 ms) the spectrum does. It is a ring of atomics the
output thread writes without allocating or locking, and only while someone watches; otherwise
one relaxed load per write. The reader takes the window being heard now (what the device has
queued plus its latency is published with the position). The analysis (`Analyzer`) is a
Hann-windowed FFT of about 170 ms (8192 points at 44.1 and 48 kHz, more at higher rates), bars
spaced logarithmically from 30 Hz to 16 kHz (bands narrower than a bin evaluated exactly at
their centre), at most one band per 1/24 octave, in dB from -72 to -12 tilted +3 dB per octave
around 1 kHz, shifted by the sensitivity. **Auto level** (on for every look): the scale slides,
within -15 to +12 dB, so the loudest bands of the last few seconds (the 85th percentile) sit at
about three quarters of the height, up within a quarter of a second, down over two and a half;
loud masters no longer pin the bars to the top, a quiet passage still moves, a drop of 20 dB
still shows as one, and near silence is never lifted. A level is then shaped (`visualizer::draw::shape`:
straight to 0.8, then bending toward 1 without reaching it). Each style has its own timing
(`visualizer::look::analysis`): the line falls 30% slower; Smooth, Normal and Snappy set
the attack (70, 25, 8 ms), the decay (420, 220, 110 ms) and the peaks' hold and fall. All
smoothing is by elapsed time, so it moves the same at any frame rate.

**When it runs.** Only while its stage is on screen: the visualizer on and Now playing shown
with that placement (the panel's tab chosen, for the panel), or Settings > Visualizer shown;
the window shown and not minimized; music playing. Then a timer at the display's refresh rate,
at most 60 a second (or 30, Settings > Frame rate) reads the tap, analyses and draws; for a
look that hears the beat (all but bars and the line with Smooth motion) the beat tracker and
its meter listen to the same samples. They start afresh when another track starts (the
engine's token for the track heard changes: a gapless one of the same album too) and when the
position jumps (back more than 0.3 s, or on more than 0.5 s past the time since the last
frame): a seek from the bar, the keys, the media controls or the lyrics alike, read from the
position the engine publishes, not from who asked for it. A tempo carried over to the next
track of a mix was weighed and not kept: until the meter has read three seconds of the new
track it cannot tell whether the tempo held, a wrong one keeps the looks still (only what falls
on its beat moves them) and no tempo lets a sure kick through, so the looks move from the new
track's first strong kicks either way. Paused or
stopped, the bars fall to rest, and then the timer, the analysis and the tap stop (the preview
goes back to its still frame); leaving the page or the section, Up next or Lyrics in the panel,
hiding or minimizing the window stops them at once. `MP_BENCH=viz-styles` checks each of these.

**Measured** on the development PC (1920 x 1032, Skia on wgpu, `MP_BENCH=viz-cost`, 5 s a case on
the "Night Shift" fixtures; process CPU, so it includes the window redrawing at 60 fps at all,
about 4% there, and the engine; playing on Now playing with the visualizer off is 0.9%).
Every case held 60 fps, frame intervals p50 16.2-16.3 ms and p95 16.5 ms; the work per frame is
the analysis and path building in Rust. Repeated runs differ by about two points either way.

| Style | Behind the page | In the panel | Full page | Work per frame (full page, p50) |
| --- | ---: | ---: | ---: | ---: |
| Bars (Classic) | 11.9% | 8.1% | 10.0% | 0.58 ms |
| Fine mirrored bars (Skyline) | | | 8.4% | 0.46 ms |
| Line (Prism) | 14.1% | 6.9% | 15.9% | 0.42 ms |
| Ring: Ripples (Halo) | 24.2% | 14.1% | 18.0% | 0.44 ms |
| Ring: Saturn | 14.1% | 10.2% | 14.1% | 0.33 ms |
| Ring: Eclipse | 20.3% | 13.3% | 19.5% | 0.41 ms |
| Bonfire | 27.3% | 14.1% | 25.0% | 0.79 ms |

The rows from the ring down are one run of 2 s a case on the `fast` profile with a game running
on the same PC (the drawing is Skia's either way), where the line measured 14.8% behind the
page, 9.4% in the panel and 17.2% on the full page: they are to be read against that, and
within three or four points.

The ring and the bonfire cost more than the other styles, most of all behind the page (three
runs each of 2 s a case on the `fast` profile, `MP_BENCH_VIZ_CASES=halo,bonfire`, with a game
running on the same PC: the same conditions put the line at 13-16% behind the page and 13-17%
on the full page). What costs is how much of the window they redraw: behind the page the
ring's stage is the cover and three tenths of it each way, and the bonfire's the whole page,
with the page's own content drawn over it every frame. Their own drawing was measured layer by
layer on the ring before it had sparks (`MP_BENCH_VIZ_CASES=halo`, each layer left out in
turn): the ripples were most of it (10.2% without them against 15.6-19.5% with), so they are a
third of the contour's points each and at most five at once; the corona and the bloom were
within the noise. The ring before its ripples travelled a whole radius and before it had sparks
measured 10.9-19.5% (median 14.1%) behind the page; the first halo (rays around the cover)
13.4-14.7%, and the plain ring before that 5.9-6.9%.

**Bars and the line with the beat** (2026-11-25, two runs before and two after, 3 s a case on
the `fast` profile, `MP_BENCH_VIZ_CASES=bars,line`): the tracker and the meter now run for them,
and the work per frame grew by about 0.03 ms (bars behind the page 0.61 to 0.63 ms, in the
panel 0.15 to 0.18 ms, the line in the panel 0.12 to 0.15-0.21 ms), a fifth of a point of a
core at 60 frames a second. Process CPU, before then after: bars behind the page 13.0, 12.5%
then 14.1, 15.1%; in the panel 9.4, 7.3% then 8.9, 9.4%; on the full page 12.5, 14.1% then
11.5, 11.5%; Skyline 11.5, 12.5% then 11.5, 13.0%; the line behind the page 16.7, 17.2% then
18.7, 16.7%; in the panel 10.4, 8.3% then 8.3, 10.4%; on the full page 15.6, 17.2% then 16.7,
19.3%. Within the runs' two points either way; bars behind the page, the default, about 1.9
points more on average.

At 30 frames a second: the line on the full page 6.6%, bars behind the page 6.2%. Before this
work the one look, Classic, measured 13.6-14.3% in `viz-idle`'s running phase (now 11.7-12.8%)
and 13.7% on the 96 kHz sweep of `visualizer`. What was tried and measured on the way: 300 fine bars with anti-aliasing 30%, on whole pixels without it 11%; the line's cost was
its strokes: with a mirrored stroke of the line in its reflection it took 21-24% on the full
page, without it 14-16% (left out; the mirrored area carries the shape), while its points 5 or
12 px apart, a faint glow under it (left out) and straight segments
sampled in Rust instead of curves each changed little by themselves. Paused and hidden after
running, the app is idle as before (`MP_BENCH=viz-idle`): 0.0 ms of CPU in 45 s in the final
runs before and after (one run of the new build saw a single 15.6 ms scheduler tick), and in a
paired run 2 context switches in 35 s before and after (counted outside, from the threads' raw
performance counters; another run of the old build saw 27). With the software renderer every style draws (gradients, the
ring's radial and conic gradients); behind the page the reflection lies under the bar, which
is opaque there (no glass), so it does not show.

**Prism.** A soundwave look with a reflection: a continuous line over the spectrum in the
prism gradient, coloured by height, a soft fill under it and a mirrored area below, with the
auto level and the +3 dB tilt. It is at its best on the full page. Skyline is the bar
counterpart: fine bars over a clear mirror.
