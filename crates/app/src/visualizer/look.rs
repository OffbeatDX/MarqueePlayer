//! The visualizer's look and where it shows: the choices Settings > Visualizer offers, the
//! presets that set them all at once, the colour schemes, and how they are saved
//! (`settings.json`, `"visualizer_look"`). Pure and unit-tested; `visualizer/mod.rs` runs it,
//! `visualizer/draw.rs` draws it.
//!
//! Precedence. Settings decide the style, colours, reflection, motion and placement. The
//! `[visualizer]` keys of theme.toml still apply: `height` is the band's height behind the
//! page, `bar_width` and `bar_gap` the bars' size at Medium detail (every preset), and
//! `opacity`, `reflection`, `reflection_opacity` and `peaks` are the Classic preset's values,
//! so a theme tuned before keeps the Classic look as it was tuned. A change made in Settings
//! wins over the file: the look becomes Custom and is saved in settings.json in full.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    /// Rounded bars with peak caps and a mirrored reflection.
    #[default]
    Bars,
    /// A smooth line over a filled area.
    Line,
    /// A contour around the cover, its ripples and its sparks. (Shockwave, a ring of
    /// sparks on each beat, was taken out: a file that names it reads as this.)
    #[serde(alias = "shockwave")]
    Radial,
    /// A low glowing line along the foot, sparks rising from it. (Fireworks, bursts of
    /// sparks on the beat, was taken out: a file that names it reads as this.)
    #[serde(alias = "fireworks")]
    Bonfire,
    /// A tilted ring of dust in orbit round the cover.
    Saturn,
    /// Around the cover: two spiral arms of dust.
    Galaxy,
    /// Around the cover: dust in a whirl, blown out by each beat.
    Swarm,
    /// Around the cover: flares arcing off the rim and back.
    Eclipse,
}

impl Style {
    /// Every style: those Settings offers side by side (`TOP`), then the other kinds of
    /// ring.
    pub const ALL: [Style; 8] = [
        Style::Bars,
        Style::Line,
        Style::Radial,
        Style::Bonfire,
        Style::Saturn,
        Style::Galaxy,
        Style::Swarm,
        Style::Eclipse,
    ];
    /// The styles as Settings offers them: everything drawn around the cover is one, "Ring".
    pub const TOP: [Style; 4] = [Style::Bars, Style::Line, Style::Radial, Style::Bonfire];
    /// The kinds of ring.
    pub const RING: [Style; 5] = [Style::Radial, Style::Saturn, Style::Galaxy, Style::Swarm, Style::Eclipse];

    pub fn label(self) -> &'static str {
        match self {
            Style::Bars => "Bars",
            Style::Line => "Line",
            Style::Radial => "Ripples",
            Style::Bonfire => "Bonfire",
            Style::Saturn => "Saturn",
            Style::Galaxy => "Galaxy",
            Style::Swarm => "Swarm",
            Style::Eclipse => "Eclipse",
        }
    }

    /// Its name where the kinds of ring are not under "Ring" (a preset's tooltip).
    pub fn full_label(self) -> String {
        if self.around() { format!("Ring: {}", self.label()) } else { self.label().to_owned() }
    }

    /// It has a reflection to show (a ring has no baseline to mirror at).
    pub fn reflects(self) -> bool {
        matches!(self, Style::Bars | Style::Line)
    }

    /// It has peaks to mark (the ring's ripples are where its contour has been).
    pub fn has_peaks(self) -> bool {
        matches!(self, Style::Bars | Style::Line)
    }

    /// It moves with the beat (the kicks and the tempo are listened for).
    pub fn beats(self) -> bool {
        self.around() || self == Style::Bonfire
    }

    /// It is drawn around the cover (round, in the stage's middle).
    pub fn around(self) -> bool {
        Style::RING.contains(&self)
    }

    /// Behind the page it has the whole page, not the band along its foot.
    pub fn tall(self) -> bool {
        self == Style::Bonfire
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Colours {
    /// The playing cover's accent, as the rest of the app.
    #[default]
    Accent,
    /// A gradient of the cover's own colours.
    Cover,
    Classic,
    Prism,
    Rainbow,
    Ember,
    Ice,
    Sunset,
    Custom,
}

impl Colours {
    pub const ALL: [Colours; 9] = [Colours::Accent, Colours::Cover, Colours::Classic, Colours::Prism, Colours::Rainbow, Colours::Ember, Colours::Ice, Colours::Sunset, Colours::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Colours::Accent => "Accent",
            Colours::Cover => "Cover",
            Colours::Classic => "Classic",
            Colours::Prism => "Prism",
            Colours::Rainbow => "Rainbow",
            Colours::Ember => "Ember",
            Colours::Ice => "Ice",
            Colours::Sunset => "Sunset",
            Colours::Custom => "Custom",
        }
    }

    /// What a screen reader and the tooltip say.
    pub fn describe(self) -> &'static str {
        match self {
            Colours::Accent => "Accent: the cover's colour, as the rest of the app",
            Colours::Cover => "Cover: a gradient of the cover's own colours",
            Colours::Classic => "Classic: green, yellow and red, like a hardware meter",
            Colours::Prism => "Prism: soft pink through yellow and mint to lavender",
            Colours::Rainbow => "Rainbow: every hue",
            Colours::Ember => "Ember: deep red to glowing orange",
            Colours::Ice => "Ice: deep blue to white",
            Colours::Sunset => "Sunset: violet, rose and peach",
            Colours::Custom => "Custom: three colours you choose",
        }
    }

    /// A fixed gradient, bottom (or left) first: (position 0..1, rgb).
    pub fn stops(self) -> Option<&'static [(f32, [u8; 3])]> {
        Some(match self {
            // Green at rest, yellow loud, red at the top: a meter's colours.
            Colours::Classic => &[(0.0, [0x00, 0xc8, 0x5a]), (0.5, [0x9c, 0xe8, 0x00]), (0.72, [0xff, 0xd4, 0x00]), (0.86, [0xff, 0x8c, 0x00]), (1.0, [0xff, 0x30, 0x24])],
            // A soft spectrum (lighter than Rainbow): pink, peach, lemon, mint, sky, lavender.
            Colours::Prism => &[
                (0.0, [0xff, 0x5c, 0x8a]),
                (0.2, [0xff, 0x9e, 0x5e]),
                (0.4, [0xff, 0xe0, 0x66]),
                (0.6, [0x7b, 0xe8, 0x9a]),
                (0.8, [0x5c, 0xc8, 0xff]),
                (1.0, [0xa8, 0x8c, 0xff]),
            ],
            Colours::Rainbow => &[
                (0.0, [0xff, 0x2d, 0x3c]),
                (0.17, [0xff, 0x8a, 0x00]),
                (0.33, [0xff, 0xe0, 0x00]),
                (0.5, [0x2e, 0xe0, 0x5a]),
                (0.67, [0x00, 0xc8, 0xff]),
                (0.83, [0x3c, 0x5c, 0xff]),
                (1.0, [0xb4, 0x3c, 0xff]),
            ],
            Colours::Ember => &[(0.0, [0x8c, 0x0a, 0x1e]), (0.4, [0xe6, 0x30, 0x14]), (0.75, [0xff, 0x8c, 0x1e]), (1.0, [0xff, 0xd8, 0x78])],
            Colours::Ice => &[(0.0, [0x1e, 0x46, 0x8c]), (0.45, [0x3c, 0x8c, 0xe6]), (0.8, [0x8c, 0xd2, 0xff]), (1.0, [0xe6, 0xf6, 0xff])],
            Colours::Sunset => &[(0.0, [0x4b, 0x23, 0x8c]), (0.4, [0xc8, 0x3c, 0x8c]), (0.75, [0xff, 0x82, 0x6e]), (1.0, [0xff, 0xc8, 0x8c])],
            Colours::Accent | Colours::Cover | Colours::Custom => return None,
        })
    }
}

/// The colours a custom gradient picks from, by name.
pub const SWATCHES: [(&str, [u8; 3]); 16] = [
    ("Red", [0xff, 0x3b, 0x3b]),
    ("Orange", [0xff, 0x8a, 0x1e]),
    ("Amber", [0xff, 0xbf, 0x1e]),
    ("Yellow", [0xff, 0xe8, 0x3c]),
    ("Lime", [0xa8, 0xe6, 0x1e]),
    ("Green", [0x2e, 0xd0, 0x64]),
    ("Teal", [0x1e, 0xc8, 0xaa]),
    ("Cyan", [0x2e, 0xd2, 0xf0]),
    ("Sky", [0x5a, 0xaa, 0xff]),
    ("Blue", [0x3c, 0x64, 0xff]),
    ("Indigo", [0x6e, 0x50, 0xf0]),
    ("Violet", [0xa0, 0x50, 0xf0]),
    ("Magenta", [0xe6, 0x3c, 0xdc]),
    ("Pink", [0xff, 0x6e, 0xaa]),
    ("White", [0xf3, 0xf0, 0xf7]),
    ("Grey", [0x8c, 0x8a, 0x96]),
];

/// The swatch a hex colour is (by name), or the nearest.
pub fn swatch_of(hex: &str) -> usize {
    let c = parse_hex(hex).unwrap_or(SWATCHES[0].1);
    let d = |a: [u8; 3]| (0..3).map(|k| (i32::from(a[k]) - i32::from(c[k])).pow(2)).sum::<i32>();
    (0..SWATCHES.len()).min_by_key(|&i| d(SWATCHES[i].1)).unwrap_or(0)
}

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColourBy {
    /// The gradient runs up the bars: quiet low, loud high.
    #[default]
    Height,
    /// The gradient runs across: each frequency its colour.
    Frequency,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    Fine,
    #[default]
    Medium,
    Coarse,
}

impl Detail {
    pub const ALL: [Detail; 3] = [Detail::Fine, Detail::Medium, Detail::Coarse];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    Smooth,
    #[default]
    Normal,
    Snappy,
}

impl Motion {
    pub const ALL: [Motion; 3] = [Motion::Smooth, Motion::Normal, Motion::Snappy];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// A band along the bottom of Now playing, behind the cover and the panel.
    #[default]
    Behind,
    /// A tab of Now playing's panel, beside Up next and Lyrics.
    Panel,
    /// The page: the cover and title in a corner.
    Page,
}

impl Placement {
    pub const ALL: [Placement; 3] = [Placement::Behind, Placement::Panel, Placement::Page];

    pub fn label(self) -> &'static str {
        match self {
            Placement::Behind => "Behind the page",
            Placement::Panel => "In the panel",
            Placement::Page => "Full page",
        }
    }

    /// As the pill says it after "Visualizer: ".
    pub fn words(self) -> &'static str {
        match self {
            Placement::Behind => "behind the page",
            Placement::Panel => "in the panel",
            Placement::Page => "full page",
        }
    }

    /// The next placement of the key that cycles them: behind, in the panel, full page, and
    /// round again.
    pub fn next(self) -> Placement {
        match self {
            Placement::Behind => Placement::Panel,
            Placement::Panel => Placement::Page,
            Placement::Page => Placement::Behind,
        }
    }
}

/// What the placement key does: with the visualizer off it comes on where it was (nothing
/// moves: the first press shows it, the next ones move it); on, the next placement. Off is
/// not a stop of the cycle (`V` turns it off). Returns (on, placement).
pub fn cycle_placement(on: bool, now: Placement) -> (bool, Placement) {
    if on { (true, now.next()) } else { (true, now) }
}

/// Everything about how it looks. Every field defaults, so an older or hand-edited file
/// loads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub style: Style,
    pub colours: Colours,
    pub colour_by: ColourBy,
    /// The custom gradient, bottom (or left) to top, as `#rrggbb`.
    pub custom: Vec<String>,
    pub reflection: bool,
    /// The reflection's height as a share of the bars' (0.1 to 1).
    pub reflection_size: f32,
    /// Its alpha at the baseline as a share of the bars' (0.05 to 1); it fades out below.
    pub reflection_strength: f32,
    pub peaks: bool,
    pub detail: Detail,
    pub motion: Motion,
    /// dB added to the music's level: up fills more of the height (quiet masters), down
    /// less (loud ones). -12 to +12.
    pub sensitivity: i32,
    /// Low notes in the middle, spreading to both sides (bars, line).
    pub mirror: bool,
    /// Its opacity behind the page (0.1 to 1), where a look is faint enough for the page to
    /// read over it. Saved under its first name, "prominence" (Settings called it "Strength
    /// behind the page"), so a file written before reads the same and an older build reads
    /// this one's.
    pub prominence: f32,
    /// Its opacity in the panel and on the full page (0.1 to 1). Every look was drawn at
    /// full strength there before it could be changed: missing from a file, it is 1.
    pub opacity: f32,
    /// The ring's swing: how far its contour reaches for the same music (0.5 to 2; 1 as
    /// designed). Missing from a file, it is 1.
    pub intensity: f32,
    /// The ring's cover pulses a little with each beat. Missing from a file, it does.
    pub cover_pulse: bool,
}

impl Default for Look {
    fn default() -> Self {
        Look::classic(&ThemeViz::default())
    }
}

/// theme.toml's `[visualizer]` keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeViz {
    pub opacity: f32,
    pub height: f32,
    pub bar_width: f32,
    pub bar_gap: f32,
    pub reflection: f32,
    pub reflection_opacity: f32,
    pub peaks: bool,
}

impl Default for ThemeViz {
    /// The built-in theme's values (theme/mod.rs, DEFAULT_THEME).
    fn default() -> Self {
        Self { opacity: 0.4, height: 200.0, bar_width: 10.0, bar_gap: 4.0, reflection: 0.45, reflection_opacity: 0.55, peaks: true }
    }
}

fn default_custom() -> Vec<String> {
    ["Blue", "Violet", "Pink"].iter().map(|n| hex(SWATCHES.iter().find(|s| s.0 == *n).unwrap().1)).collect()
}

impl Look {
    /// The look the visualizer had before it had choices: bars in the accent, behind the
    /// page, faint, with caps; and its reflection, now where it can be seen.
    pub fn classic(t: &ThemeViz) -> Self {
        Look {
            style: Style::Bars,
            colours: Colours::Accent,
            colour_by: ColourBy::Height,
            custom: default_custom(),
            reflection: t.reflection > 0.0,
            reflection_size: t.reflection.clamp(0.1, 1.0),
            reflection_strength: t.reflection_opacity.clamp(0.05, 1.0),
            peaks: t.peaks,
            detail: Detail::Medium,
            motion: Motion::Normal,
            sensitivity: 0,
            mirror: false,
            prominence: t.opacity.clamp(0.1, 1.0),
            opacity: 1.0,
            intensity: 1.0,
            cover_pulse: true,
        }
    }

    /// The beat is listened for: the looks that move with it, and bars and the line, whose
    /// low notes take the kick and whose light swells with the beat, unless their motion is
    /// Smooth (Calm, and the default where Windows' animations are off: no jumping there).
    pub fn hears_beat(&self) -> bool {
        self.style.beats() || matches!(self.style, Style::Bars | Style::Line) && self.motion != Motion::Smooth
    }

    /// How strongly it is drawn where it shows: behind the page, or elsewhere (the panel,
    /// the full page).
    pub fn strength(&self, behind: bool) -> f32 {
        if behind { self.prominence } else { self.opacity }
    }

    /// The same look, its fractions compared to a hair (a value read back from text or a
    /// file may differ from the one stepped to in the last bit).
    pub fn same(&self, other: &Look) -> bool {
        let near = |a: f32, b: f32| (a - b).abs() < 0.004;
        let a = self.clone().clamped();
        let b = other.clone().clamped();
        near(a.reflection_size, b.reflection_size)
            && near(a.reflection_strength, b.reflection_strength)
            && near(a.prominence, b.prominence)
            && near(a.opacity, b.opacity)
            && near(a.intensity, b.intensity)
            && Look { reflection_size: 0.0, reflection_strength: 0.0, prominence: 0.0, opacity: 0.0, intensity: 0.0, ..a } == Look { reflection_size: 0.0, reflection_strength: 0.0, prominence: 0.0, opacity: 0.0, intensity: 0.0, ..b }
    }

    /// Values brought into their ranges (a hand-edited file).
    pub fn clamped(mut self) -> Self {
        self.reflection_size = finite(self.reflection_size, 0.45).clamp(0.1, 1.0);
        self.reflection_strength = finite(self.reflection_strength, 0.5).clamp(0.05, 1.0);
        self.sensitivity = self.sensitivity.clamp(-12, 12);
        self.prominence = finite(self.prominence, 0.4).clamp(0.1, 1.0);
        self.opacity = finite(self.opacity, 1.0).clamp(0.1, 1.0);
        self.intensity = finite(self.intensity, 1.0).clamp(0.5, 2.0);
        self.custom.truncate(3);
        while self.custom.len() < 3 {
            self.custom.push(default_custom()[self.custom.len()].clone());
        }
        for c in &mut self.custom {
            if parse_hex(c).is_none() {
                *c = hex(SWATCHES[swatch_of(c)].1);
            }
        }
        self
    }
}

fn finite(v: f32, or: f32) -> f32 {
    if v.is_finite() { v } else { or }
}

/// A preset: a whole look by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Classic,
    Prism,
    Skyline,
    Calm,
    Bonfire,
    Halo,
    Saturn,
    Galaxy,
    Swarm,
    Eclipse,
}

impl Preset {
    /// In the order they are offered: the bars and the lines, the bonfire, then the looks
    /// drawn around the cover.
    pub const ALL: [Preset; 10] = [
        Preset::Classic,
        Preset::Prism,
        Preset::Skyline,
        Preset::Calm,
        Preset::Bonfire,
        Preset::Halo,
        Preset::Saturn,
        Preset::Galaxy,
        Preset::Swarm,
        Preset::Eclipse,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Preset::Classic => "classic",
            Preset::Prism => "prism",
            Preset::Halo => "halo",
            Preset::Bonfire => "bonfire",
            Preset::Saturn => "saturn",
            Preset::Galaxy => "galaxy",
            Preset::Swarm => "swarm",
            Preset::Eclipse => "eclipse",
            Preset::Skyline => "skyline",
            Preset::Calm => "calm",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::Classic => "Classic",
            Preset::Prism => "Prism",
            Preset::Halo => "Halo",
            Preset::Bonfire => "Bonfire",
            Preset::Saturn => "Saturn",
            Preset::Galaxy => "Galaxy",
            Preset::Swarm => "Swarm",
            Preset::Eclipse => "Eclipse",
            Preset::Skyline => "Skyline",
            Preset::Calm => "Calm",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Preset::Classic => "Bars in the cover's colour, quietly behind the page",
            Preset::Prism => "A bright line in a soft spectrum over its reflection",
            Preset::Halo => "A glowing ring around the cover, in its colours: ripples and sparks on the beat",
            Preset::Bonfire => "Sparks rising from a glowing line along the foot, in the cover's colours",
            Preset::Saturn => "A tilted ring of dust in orbit round the cover, passing behind it and in front",
            Preset::Galaxy => "Two spiral arms of dust turning round the cover, winding tighter with the bass",
            Preset::Swarm => "Dust in a whirl round the cover: every beat blows it outward and it spirals back",
            Preset::Eclipse => "A glow at the cover's rim, and on each beat a flare of sparks arcing out and back",
            Preset::Skyline => "Fine bars, low notes in the middle, over a clear mirror",
            Preset::Calm => "A slow, soft line: little movement, for a quiet room or less motion",
        }
    }

    pub fn from_id(id: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.id() == id)
    }

    pub fn look(self, t: &ThemeViz) -> Look {
        let base = Look::classic(t);
        match self {
            Preset::Classic => base,
            // A line in the prism gradient, coloured by height, its reflection below.
            Preset::Prism => Look {
                style: Style::Line,
                colours: Colours::Prism,
                reflection_size: 0.6,
                reflection_strength: 0.35,
                peaks: false,
                detail: Detail::Medium,
                prominence: 0.85,
                ..base
            },
            // Around the cover, in its colours round the ring; behind the page it is under the
            // cover itself, so nearly as strong there.
            Preset::Halo => Look { style: Style::Radial, colours: Colours::Cover, colour_by: ColourBy::Frequency, reflection: false, peaks: false, detail: Detail::Medium, prominence: 0.85, ..base },
            // Sparks over the whole page behind it: nearly as strong as the halo, since most
            // of it is dark between them.
            Preset::Bonfire => Look { style: Style::Bonfire, colours: Colours::Cover, colour_by: ColourBy::Frequency, reflection: false, peaks: false, detail: Detail::Medium, prominence: 0.8, ..base },
            // The other looks drawn around the cover: the halo's, with their own drawing.
            Preset::Saturn => Look { style: Style::Saturn, ..Preset::Halo.look(t) },
            Preset::Galaxy => Look { style: Style::Galaxy, ..Preset::Halo.look(t) },
            Preset::Swarm => Look { style: Style::Swarm, ..Preset::Halo.look(t) },
            Preset::Eclipse => Look { style: Style::Eclipse, ..Preset::Halo.look(t) },
            Preset::Skyline => Look {
                style: Style::Bars,
                colours: Colours::Cover,
                colour_by: ColourBy::Height,
                reflection_size: 0.7,
                reflection_strength: 0.6,
                peaks: false,
                detail: Detail::Fine,
                mirror: true,
                prominence: 0.7,
                ..base
            },
            Preset::Calm => Look {
                style: Style::Line,
                colours: Colours::Accent,
                reflection_size: 0.5,
                reflection_strength: 0.3,
                peaks: false,
                detail: Detail::Coarse,
                motion: Motion::Smooth,
                sensitivity: -3,
                prominence: 0.35,
                ..base
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fps {
    /// The display's refresh rate, at most 60.
    #[default]
    Full,
    /// 30 frames a second: half the work, for laptops.
    Half,
}

/// What settings.json keeps (`"visualizer_look"`). The on/off switch is `visualizer`, apart;
/// the user's own presets are `visualizer_presets` (`UserPresets`), apart too, so Reset
/// (which forgets this) keeps them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VizSettings {
    /// The preset in use by id, or "custom": then `look` is the look, in full. One of the
    /// user's presets is "custom" too, with its look (an older build shows it as it is).
    pub preset: String,
    pub look: Look,
    pub placement: Placement,
    pub fps: Fps,
    /// The user's preset last chosen, by name, while the look is (or came from) it: what
    /// "Update" saves to after a change. `None` for a built-in one or none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub based_on: Option<String>,
}

impl Default for VizSettings {
    fn default() -> Self {
        Self { preset: Preset::Classic.id().into(), look: Look::default(), placement: Placement::Behind, fps: Fps::Full, based_on: None }
    }
}

impl VizSettings {
    /// The look in effect: the preset's (Classic's taking theme.toml's values), or the
    /// custom one. An unknown preset (a newer build's) is Classic.
    pub fn effective(&self, t: &ThemeViz) -> Look {
        if self.preset == "custom" {
            return self.look.clone().clamped();
        }
        // A preset this build does not have (one taken out since; a newer
        // build's): the look it left in the file, as a look of the user's own.
        match Preset::from_id(&self.preset) {
            Some(p) => p.look(t),
            None => self.look.clone().clamped(),
        }
    }

    /// The preset in use, if any.
    pub fn preset(&self) -> Option<Preset> {
        Preset::from_id(&self.preset)
    }

    pub fn choose(&mut self, p: Preset, t: &ThemeViz) {
        self.preset = p.id().into();
        self.look = p.look(t);
        self.based_on = None;
    }

    /// One of the user's presets: its look, in full.
    pub fn choose_user(&mut self, p: &UserLook) {
        self.preset = "custom".into();
        self.look = p.look.clone().clamped();
        self.based_on = Some(p.name.clone());
    }

    /// Change one thing about the look in effect. The result is the built-in preset it
    /// equals, if one, else Custom (still based on the user's preset it came from).
    pub fn change(&mut self, t: &ThemeViz, f: impl FnOnce(&mut Look)) {
        let mut look = self.effective(t);
        f(&mut look);
        let look = look.clamped();
        match Preset::ALL.into_iter().find(|p| p.look(t).same(&look)) {
            Some(p) => self.choose(p, t),
            None => {
                self.preset = "custom".into();
                self.look = look;
            }
        }
    }

    /// What the look is now, among the presets: a built-in one, or one of the user's (the
    /// one it is based on first, then the first with this look), or none (Custom).
    pub fn current(&self, t: &ThemeViz, users: &[UserLook]) -> Option<Pick> {
        if let Some(p) = self.preset() {
            return Some(Pick::BuiltIn(p));
        }
        let look = self.effective(t);
        let based = self.based_on.as_deref().and_then(|n| users.iter().position(|u| u.name == n)).filter(|&i| users[i].look.same(&look));
        based.or_else(|| users.iter().position(|u| u.look.same(&look))).map(Pick::User)
    }

    /// The next look of the key that cycles them: the built-in presets, then the user's, in
    /// their order, and round again. From Custom, the first.
    pub fn next_pick(&self, t: &ThemeViz, users: &[UserLook]) -> Pick {
        let total = Preset::ALL.len() + users.len();
        let at = match self.current(t, users) {
            Some(Pick::BuiltIn(p)) => Preset::ALL.iter().position(|q| *q == p).unwrap_or(0),
            Some(Pick::User(i)) => Preset::ALL.len() + i,
            None => return Pick::BuiltIn(Preset::ALL[0]),
        };
        let next = (at + 1) % total;
        if next < Preset::ALL.len() { Pick::BuiltIn(Preset::ALL[next]) } else { Pick::User(next - Preset::ALL.len()) }
    }
}

/// A look among the presets: built in, or the user's (by its index in their list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    BuiltIn(Preset),
    User(usize),
}

/// One of the user's presets: a name and a look (only the look: not where it shows, not
/// whether it is on).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserLook {
    pub name: String,
    pub look: Look,
}

/// The longest name a preset keeps (in characters).
pub const NAME_MAX: usize = 40;

/// A name as a preset keeps it: one line, spaces collapsed, no control characters, at most
/// `NAME_MAX` characters.
pub fn clean_name(s: &str) -> String {
    let one: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let words = one.split_whitespace().collect::<Vec<_>>().join(" ");
    words.chars().take(NAME_MAX).collect::<String>().trim_end().to_owned()
}

/// Two names are the same preset's (case and outer spaces aside).
pub fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// `name`, or "name (2)", "(3)"... the first that is not taken.
pub fn unique_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    (2..1000).map(|i| format!("{name} ({i})")).find(|n| !taken(n)).unwrap_or_else(|| name.to_owned())
}

/// The user's presets (`"visualizer_presets"` in settings.json), in the order made. Read
/// one by one: an entry this build cannot read (a newer build's style, a damaged one) is
/// kept as it was and written back so, never costing the others or the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserPresets {
    pub list: Vec<UserLook>,
    /// Entries this build could not read, as stored.
    pub kept: Vec<serde_json::Value>,
}

impl UserPresets {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.kept.is_empty()
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.list.iter().position(|p| same_name(&p.name, name))
    }

    /// A name a preset has, built in or the user's.
    pub fn taken(&self, name: &str) -> bool {
        self.find(name).is_some() || Preset::ALL.iter().any(|p| same_name(p.label(), name))
    }
}

impl Serialize for UserPresets {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(self.list.len() + self.kept.len()))?;
        for p in &self.list {
            seq.serialize_element(p)?;
        }
        for v in &self.kept {
            seq.serialize_element(v)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for UserPresets {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        let mut out = UserPresets::default();
        let serde_json::Value::Array(items) = v else {
            if !v.is_null() {
                log::warn!("settings.json: visualizer_presets is not a list; ignored");
            }
            return Ok(out);
        };
        for item in items {
            match serde_json::from_value::<UserLook>(item.clone()) {
                Ok(p) if !clean_name(&p.name).is_empty() && out.find(&p.name).is_none() => out.list.push(UserLook { name: clean_name(&p.name), look: p.look.clamped() }),
                _ => {
                    log::warn!("settings.json: a visualizer preset this version cannot read is kept as it is");
                    out.kept.push(item);
                }
            }
        }
        Ok(out)
    }
}

/// How it moves for a look: the analyser's timing and range (`spectrum::Config`).
pub fn analysis(look: &Look) -> player_core::spectrum::Config {
    let (attack, decay, hold, fall) = match look.motion {
        Motion::Smooth => (0.07, 0.42, 0.8, 1.2),
        Motion::Normal => (0.025, 0.22, 0.5, 2.5),
        Motion::Snappy => (0.008, 0.11, 0.35, 5.0),
    };
    // A line reads as one shape: a touch slower falls keep it fluid.
    // The ring moves as bars do: slowed down (as it once was) it hardly moved at all.
    let decay = decay
        * match look.style {
            Style::Line => 1.3,
            _ => 1.0,
        };
    let s = look.sensitivity as f32;
    let d = player_core::spectrum::Config::default();
    player_core::spectrum::Config { min_db: d.min_db - s, max_db: d.max_db - s, attack, decay, peak_hold: hold, peak_fall: fall, auto_level: true, ..d }
}

/// The frame interval: the display's refresh period, at most 60 a second (or 30).
pub fn frame_interval(fps: Fps, refresh_hz: Option<u32>) -> std::time::Duration {
    let cap = match fps {
        Fps::Full => 60,
        Fps::Half => 30,
    };
    let hz = refresh_hz.filter(|h| (24..=1000).contains(h)).unwrap_or(60).min(cap);
    // Whole milliseconds, rounded down: a timer a hair early lines up with the next vsync
    // rather than missing it.
    std::time::Duration::from_millis((1000 / u64::from(hz)).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_their_defaults() {
        let s: VizSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, VizSettings::default());
        let s: VizSettings = serde_json::from_str(r#"{ "preset": "custom", "look": { "style": "line" } }"#).unwrap();
        assert_eq!(s.look.style, Style::Line);
        assert_eq!(s.look.colours, Colours::Accent, "the rest as Classic");
        assert_eq!(s.placement, Placement::Behind);
        assert_eq!(s.fps, Fps::Full);
    }

    #[test]
    fn looks_taken_out_read_as_their_nearest_relative() {
        // Shockwave was a kind of ring, Fireworks sparks on the beat: a file that names one
        // still loads, as Ripples and as the bonfire, and is written back as those.
        let s: VizSettings = serde_json::from_str(r#"{ "preset": "custom", "look": { "style": "shockwave" }, "placement": "page" }"#).unwrap();
        assert_eq!((s.look.style, s.placement), (Style::Radial, Placement::Page));
        let s: VizSettings = serde_json::from_str(r#"{ "preset": "fireworks", "look": { "style": "fireworks", "colours": "cover" } }"#).unwrap();
        assert_eq!((s.preset(), s.effective(&ThemeViz::default()).style), (None, Style::Bonfire));
        assert!(serde_json::to_string(&s).unwrap().contains("\"style\":\"bonfire\""));
        // Every look drawn around the cover has a preset of its own.
        let t = ThemeViz::default();
        assert!(Style::RING.iter().all(|k| Preset::ALL.iter().any(|p| p.look(&t).style == *k)));
    }

    #[test]
    fn options_round_trip() {
        let mut s = VizSettings::default();
        let t = ThemeViz::default();
        s.change(&t, |l| {
            l.style = Style::Radial;
            l.colours = Colours::Custom;
            l.custom = vec!["#112233".into(), "#445566".into(), "#778899".into()];
            l.sensitivity = 6;
        });
        s.placement = Placement::Page;
        s.fps = Fps::Half;
        let text = serde_json::to_string(&s).unwrap();
        let back: VizSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.preset, "custom");
        assert_eq!(back.effective(&t).custom[1], "#445566");
    }

    #[test]
    fn unknown_values_never_fail_the_file() {
        // A newer build's preset is Classic here; a value out of range is brought in.
        let s: VizSettings = serde_json::from_str(r#"{ "preset": "aurora" }"#).unwrap();
        assert_eq!(s.effective(&ThemeViz::default()), Look::classic(&ThemeViz::default()));
        let s: VizSettings = serde_json::from_str(r#"{ "preset": "custom", "look": { "sensitivity": 99, "prominence": 7, "custom": ["nonsense"] } }"#).unwrap();
        let l = s.effective(&ThemeViz::default());
        assert_eq!((l.sensitivity, l.prominence, l.custom.len()), (12, 1.0, 3));
        assert!(parse_hex(&l.custom[0]).is_some());
    }

    #[test]
    fn theme_toml_sets_classic_and_settings_win() {
        let t = ThemeViz { opacity: 0.25, reflection: 0.3, reflection_opacity: 0.8, peaks: false, ..ThemeViz::default() };
        let mut s = VizSettings::default();
        let l = s.effective(&t);
        assert_eq!((l.prominence, l.reflection_size, l.reflection_strength, l.peaks), (0.25, 0.3, 0.8, false), "Classic is the theme's");
        // Another preset is its own, whatever the theme says.
        s.choose(Preset::Prism, &t);
        assert_eq!(s.effective(&t).reflection_size, 0.6);
        // A preset this build no longer has: the look the file kept, as the
        // user's own, not Classic.
        let old = VizSettings { preset: "retro".into(), look: Look { style: Style::Line, colours: Colours::Classic, ..Look::default() }, ..VizSettings::default() };
        assert_eq!((old.preset(), old.effective(&t).style, old.effective(&t).colours), (None, Style::Line, Colours::Classic));
        // A change in Settings wins: the look becomes Custom, the file no longer applies.
        s.choose(Preset::Classic, &t);
        s.change(&t, |l| l.prominence = 0.6);
        assert_eq!(s.preset, "custom");
        let other = ThemeViz { opacity: 0.9, ..t };
        assert_eq!(s.effective(&other).prominence, 0.6);
        // Changing a value back to the preset's is the preset again.
        s.change(&t, |l| l.prominence = 0.25);
        assert_eq!(s.preset, "classic");
        // A reflection height of 0 in theme.toml turns Classic's reflection off.
        assert!(!Look::classic(&ThemeViz { reflection: 0.0, ..ThemeViz::default() }).reflection);
    }

    #[test]
    fn presets_are_distinct_and_cycle() {
        let t = ThemeViz::default();
        for (i, a) in Preset::ALL.iter().enumerate() {
            assert_eq!(Preset::from_id(a.id()), Some(*a));
            for b in &Preset::ALL[i + 1..] {
                assert!(!a.look(&t).same(&b.look(&t)), "{a:?} and {b:?}");
            }
        }
        let mut s = VizSettings::default();
        let mut seen = vec![s.preset().unwrap()];
        for _ in 0..Preset::ALL.len() - 1 {
            let Pick::BuiltIn(p) = s.next_pick(&t, &[]) else { panic!("built in") };
            s.choose(p, &t);
            seen.push(p);
        }
        assert_eq!(seen, Preset::ALL);
        assert_eq!(s.next_pick(&t, &[]), Pick::BuiltIn(Preset::Classic), "wraps");
        s.change(&t, |l| l.sensitivity = 9);
        assert_eq!(s.next_pick(&t, &[]), Pick::BuiltIn(Preset::Classic), "from Custom, the first");
    }

    fn mine(name: &str, f: impl FnOnce(&mut Look)) -> UserLook {
        let mut look = Preset::Halo.look(&ThemeViz::default());
        f(&mut look);
        UserLook { name: name.into(), look }
    }

    #[test]
    fn the_look_key_goes_through_the_built_in_presets_then_the_users() {
        let t = ThemeViz::default();
        let users = vec![mine("Night", |l| l.sensitivity = 6), mine("Day", |l| l.colours = Colours::Sunset)];
        let mut s = VizSettings::default();
        s.choose(Preset::Eclipse, &t);
        assert_eq!(s.next_pick(&t, &users), Pick::User(0), "after the last built-in one, the first of the user's");
        s.choose_user(&users[0]);
        assert_eq!(s.current(&t, &users), Some(Pick::User(0)));
        assert_eq!(s.based_on.as_deref(), Some("Night"));
        assert_eq!(s.next_pick(&t, &users), Pick::User(1));
        s.choose_user(&users[1]);
        assert_eq!(s.next_pick(&t, &users), Pick::BuiltIn(Preset::Classic), "and round again");
        // Changed: Custom, still based on it (what Update saves to); changed back: it again.
        s.change(&t, |l| l.peaks = !l.peaks);
        assert_eq!((s.current(&t, &users), s.based_on.as_deref()), (None, Some("Day")));
        s.change(&t, |l| l.peaks = !l.peaks);
        assert_eq!(s.current(&t, &users), Some(Pick::User(1)));
        // A look equal to a built-in one is that one.
        s.choose_user(&mine("Like Halo", |_| {}));
        assert_eq!(s.current(&t, &users), None, "Like Halo is not in the list");
        s.change(&t, |_| {});
        assert_eq!(s.current(&t, &users), Some(Pick::BuiltIn(Preset::Halo)));
    }

    #[test]
    fn opacity_is_kept_behind_the_page_and_new_elsewhere() {
        // A file from before: "prominence" was the strength behind the page; the panel and
        // the full page were always at full strength. Read the same.
        let before = r#"{ "preset": "custom", "look": { "style": "line", "prominence": 0.6 }, "placement": "panel" }"#;
        let s: VizSettings = serde_json::from_str(before).unwrap();
        let l = s.effective(&ThemeViz::default());
        assert_eq!((l.strength(true), l.strength(false)), (0.6, 1.0), "behind as it was, full elsewhere");
        // Every built-in preset is as it was: its own strength behind the page, full elsewhere.
        for p in Preset::ALL {
            let l = p.look(&ThemeViz::default());
            assert_eq!(l.opacity, 1.0, "{p:?}");
            assert!(l.prominence < 1.0, "{p:?}");
        }
        // Written with both, under the first one's name, so an older build reads it too.
        let mut s = s;
        s.change(&ThemeViz::default(), |l| l.opacity = 0.7);
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"prominence\":0.6") && text.contains("\"opacity\":0.7"), "{text}");
        let back: VizSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back, s);
        // Out of range or nonsense is brought in.
        let s: VizSettings = serde_json::from_str(r#"{ "preset": "custom", "look": { "opacity": 0.0 } }"#).unwrap();
        assert_eq!(s.effective(&ThemeViz::default()).opacity, 0.1);
    }

    #[test]
    fn the_placement_key_cycles_and_turns_it_on() {
        assert_eq!(Placement::Behind.next(), Placement::Panel);
        assert_eq!(Placement::Panel.next(), Placement::Page);
        assert_eq!(Placement::Page.next(), Placement::Behind, "and round");
        // Off: on, where it was (the first press shows it; nothing moves).
        assert_eq!(cycle_placement(false, Placement::Page), (true, Placement::Page));
        assert_eq!(cycle_placement(true, Placement::Page), (true, Placement::Behind));
        let mut p = Placement::Behind;
        let mut seen = Vec::new();
        for _ in 0..6 {
            p = cycle_placement(true, p).1;
            seen.push(p);
        }
        assert_eq!(seen, [Placement::Panel, Placement::Page, Placement::Behind, Placement::Panel, Placement::Page, Placement::Behind]);
        assert!(Placement::ALL.iter().all(|p| !p.words().is_empty()));
    }

    #[test]
    fn user_presets_are_read_one_by_one_and_kept() {
        let text = r#"[
            { "name": "Night", "look": { "style": "radial", "colours": "cover", "opacity": 0.8 } },
            { "name": "From the future", "look": { "style": "plasma" } },
            { "name": "  Spaced\u0007  out  ", "look": {} },
            { "name": "night", "look": {} },
            "not a preset",
            { "look": { "style": "bars" } }
        ]"#;
        let p: UserPresets = serde_json::from_str(text).unwrap();
        assert_eq!(p.list.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["Night", "Spaced out"]);
        assert_eq!(p.list[0].look.style, Style::Radial);
        assert_eq!(p.kept.len(), 4, "a newer style, a second Night, junk, no name: kept as they were");
        // Written back: the readable ones, then the kept ones untouched.
        let again = serde_json::to_value(&p).unwrap();
        assert_eq!(again.as_array().unwrap().len(), 6);
        assert_eq!(again[2]["look"]["style"], "plasma");
        let back: UserPresets = serde_json::from_value(again).unwrap();
        assert_eq!(back, p);
        // Not a list: none, said in the log; never the file.
        let p: UserPresets = serde_json::from_str(r#"{"oops": 1}"#).unwrap();
        assert!(p.is_empty());
        // Names: built-in ones are taken, case does not matter.
        let p: UserPresets = serde_json::from_str(r#"[{ "name": "Mine", "look": {} }]"#).unwrap();
        assert!(p.taken("mine") && p.taken("HALO") && !p.taken("Yours"));
        assert_eq!(unique_name("Mine", |n| p.taken(n)), "Mine (2)");
        assert_eq!(clean_name(&format!("  a\n\tb {}", "x".repeat(80))).chars().count(), NAME_MAX);
    }

    #[test]
    fn every_style_and_colour_has_a_label() {
        assert!(Style::ALL.iter().all(|s| !s.label().is_empty()));
        for c in Colours::ALL {
            assert!(!c.label().is_empty() && c.describe().starts_with(c.label()));
            if let Some(stops) = c.stops() {
                assert_eq!(stops.first().unwrap().0, 0.0);
                assert_eq!(stops.last().unwrap().0, 1.0);
                assert!(stops.windows(2).all(|w| w[0].0 < w[1].0), "{c:?} in order");
            }
        }
        assert_eq!(swatch_of("#ff3b3b"), 0);
        assert_eq!(SWATCHES[swatch_of(&hex([0x3c, 0x64, 0xfe]))].0, "Blue", "the nearest");
    }

    #[test]
    fn motion_and_sensitivity_map_to_the_analyser() {
        let mut l = Look::default();
        let normal = analysis(&l);
        l.motion = Motion::Smooth;
        let smooth = analysis(&l);
        assert!(smooth.decay > normal.decay && smooth.attack > normal.attack);
        l.sensitivity = 6;
        let s = analysis(&l);
        assert_eq!((s.min_db, s.max_db), (normal.min_db - 6.0, normal.max_db - 6.0));
    }

    #[test]
    fn frames_follow_the_display_up_to_the_cap() {
        use std::time::Duration;
        assert_eq!(frame_interval(Fps::Full, Some(60)), Duration::from_millis(16));
        assert_eq!(frame_interval(Fps::Full, Some(144)), Duration::from_millis(16), "never past 60");
        assert_eq!(frame_interval(Fps::Full, Some(50)), Duration::from_millis(20));
        assert_eq!(frame_interval(Fps::Half, Some(144)), Duration::from_millis(33));
        assert_eq!(frame_interval(Fps::Full, None), Duration::from_millis(16));
        assert_eq!(frame_interval(Fps::Full, Some(1)), Duration::from_millis(16), "nonsense is ignored");
    }
}
