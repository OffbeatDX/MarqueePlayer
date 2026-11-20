//! The smart playlist editor's draft: the rule set as the user is typing it, and the lines
//! the editor shows for it (`ui/smart.slint`). Pure logic, unit-tested; `playlists/smart.rs` keeps the
//! draft, asks for its preview and saves it.
//!
//! Each condition is a line that reads as a sentence: its field, its operator and its value,
//! each a control of its own (`Slot`): a choice that opens a list (fields, operators, units,
//! values the library holds: artists, genres, labels, formats, playlists), a typed field
//! (text, numbers, lengths, dates), stars, words between them ("and", "kbps"). What is typed
//! is kept as typed and read the forgiving way (`3:30`, `44.1`, `2024-03`), and a value that
//! cannot be read is explained under its line, not taken. Groups ("any of these") are one
//! level deep here; what a newer build wrote stays as it was, shown as a line of its own.

use library::smart::{self, Cond, DateSpec, Field, Group, Kind, Limit, LimitUnit, Limiting, Node, Op, Smart, Sort, SortKey, Sorting, Unit, Value};
use serde_json::{Map, Value as Json};

// What a line is (`SmartRow.kind`).
pub const RULE: i32 = 0;
pub const GROUP: i32 = 1;
pub const UNKNOWN: i32 = 2;
pub const SORT: i32 = 3;
pub const LIMIT: i32 = 4;
pub const JOIN: i32 = 5;

// What a control is (`SmartSlot.kind`).
pub const CHOICE: i32 = 0;
pub const TEXT: i32 = 1;
pub const WORDS: i32 = 2;
pub const STARS: i32 = 3;
pub const SWITCH: i32 = 4;

/// What a control sets (`SmartSlot.role`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Field,
    Op,
    A,
    B,
    Unit,
    Within,
    WithinUnit,
    Join,
    SortKey,
    SortDir,
    SortN,
    SortUnit,
    LimitOn,
    LimitN,
    LimitUnit,
    ByKey,
    ByDir,
    ByN,
    ByUnit,
    Words,
}

impl Role {
    const ALL: [Role; 20] = [
        Role::Field,
        Role::Op,
        Role::A,
        Role::B,
        Role::Unit,
        Role::Within,
        Role::WithinUnit,
        Role::Join,
        Role::SortKey,
        Role::SortDir,
        Role::SortN,
        Role::SortUnit,
        Role::LimitOn,
        Role::LimitN,
        Role::LimitUnit,
        Role::ByKey,
        Role::ByDir,
        Role::ByN,
        Role::ByUnit,
        Role::Words,
    ];

    pub fn index(self) -> i32 {
        Role::ALL.iter().position(|r| *r == self).unwrap_or(0) as i32
    }

    pub fn from_index(i: i32) -> Option<Role> {
        Role::ALL.get(usize::try_from(i).ok()?).copied()
    }
}

/// One condition as typed.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub field: Field,
    pub op: Op,
    /// The value as typed or picked (a text, a number, a length, a date, how many).
    pub a: String,
    /// The second value of "between".
    pub b: String,
    /// "In the last N <unit>".
    pub unit: Unit,
    /// Plays in the last <within> <within_unit>.
    pub within: String,
    pub within_unit: Unit,
    /// A playlist (membership).
    pub id: i64,
    /// Touched since it was added: its problems show only then (a new line is not an error).
    pub touched: bool,
}

impl Line {
    /// A new line on `field`, with a value that makes sense to start from.
    pub fn new(field: Field) -> Line {
        let mut l = Line { field, op: field.default_op(), a: String::new(), b: String::new(), unit: Unit::Months, within: "30".into(), within_unit: Unit::Days, id: 0, touched: false };
        l.a = default_value(field, l.op);
        l
    }

    fn from_cond(c: &Cond) -> Line {
        let mut l = Line::new(c.field);
        l.op = c.op;
        l.touched = true;
        match &c.value {
            Value::Text(t) => l.a = t.clone(),
            Value::Num(v) => l.a = number_text(c.field, *v),
            Value::Range(x, y) => {
                l.a = number_text(c.field, *x);
                l.b = number_text(c.field, *y);
            }
            Value::Date(d) => l.a = d.iso(),
            Value::Dates(x, y) => {
                l.a = x.iso();
                l.b = y.iso();
            }
            Value::Ago(n, u) => {
                l.a = n.to_string();
                l.unit = *u;
            }
            Value::Id(id) => l.id = *id,
            Value::None => l.a.clear(),
        }
        if let Some((n, u)) = c.within {
            l.within = n.to_string();
            l.within_unit = u;
        }
        l
    }

    /// The condition it says, or why it says none yet.
    pub fn cond(&self) -> Result<Cond, String> {
        let kind = self.field.kind();
        let value = match (kind, self.op) {
            (Kind::Text | Kind::Choice, _) => Value::Text(self.a.clone()),
            (Kind::Number, Op::Between) => Value::Range(parse_number(self.field, &self.a)?, parse_number(self.field, &self.b)?),
            (Kind::Number, _) => Value::Num(parse_number(self.field, &self.a)?),
            (Kind::Date, Op::InLast | Op::NotInLast) => Value::Ago(parse_count(&self.a).ok_or("Type how many")?, self.unit),
            (Kind::Date, Op::Between) => Value::Dates(parse_date(&self.a)?, parse_date(&self.b)?),
            (Kind::Date, _) => Value::Date(parse_date(&self.a)?),
            (Kind::Bool, _) => Value::None,
            (Kind::Playlist, _) => Value::Id(self.id),
        };
        let within = if self.field == Field::PlaysIn { Some((parse_count(&self.within).ok_or("Say how far back to count plays")?, self.within_unit)) } else { None };
        let c = Cond { field: self.field, op: self.op, value, within };
        c.check()?;
        Ok(c)
    }
}

fn default_value(field: Field, op: Op) -> String {
    match (field, op) {
        (Field::Rating, _) => "4".into(),
        (Field::Plays | Field::PlaysIn, _) => "1".into(),
        (Field::Duration, _) => "5:00".into(),
        (Field::Bitrate, _) => "320".into(),
        (Field::SampleRate, _) => "96".into(),
        (Field::BitDepth, _) => "24".into(),
        (Field::Disc | Field::TrackNo, _) => "1".into(),
        (_, Op::InLast | Op::NotInLast) => "1".into(),
        _ => String::new(),
    }
}

/// A field's number as the editor shows it to edit: lengths "3:30", sample rates in kHz.
fn number_text(field: Field, v: f64) -> String {
    smart::shown_number(field, v)
}

/// A count typed: a whole number 1 or more.
fn parse_count(s: &str) -> Option<u32> {
    s.trim().parse::<u32>().ok().filter(|n| (1..=100_000).contains(n))
}

fn parse_date(s: &str) -> Result<DateSpec, String> {
    if s.trim().is_empty() {
        return Err("Type a date, like 2024-03-15 (or 2024-03, or 2024)".into());
    }
    DateSpec::parse(s).ok_or_else(|| format!("\u{201c}{}\u{201d} is not a date: type it like 2024-03-15 (or 2024-03, or 2024)", s.trim()))
}

/// A number typed for a field, in the field's own units: a length as "3:30", "1:02:03" or
/// minutes ("5"); a sample rate in kHz ("44.1", or Hz: "44100"); the rest whole numbers.
pub fn parse_number(field: Field, s: &str) -> Result<f64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(match field {
            Field::Duration => "Type a length, like 3:30".into(),
            Field::Year => "Type a year".into(),
            _ => "Type a number".into(),
        });
    }
    match field {
        Field::Duration => {
            let parts: Vec<&str> = s.split(':').collect();
            let nums: Option<Vec<u64>> = parts.iter().map(|p| p.trim().parse::<u64>().ok()).collect();
            let nums = nums.ok_or_else(|| format!("\u{201c}{s}\u{201d} is not a length: type it like 3:30"))?;
            let secs = match nums.as_slice() {
                [m] => m * 60,
                [m, sec] if *sec < 60 => m * 60 + sec,
                [h, m, sec] if *m < 60 && *sec < 60 => h * 3600 + m * 60 + sec,
                _ => return Err(format!("\u{201c}{s}\u{201d} is not a length: type it like 3:30")),
            };
            Ok(secs as f64)
        }
        Field::SampleRate => {
            let v: f64 = s.trim_end_matches("kHz").trim().replace(',', ".").parse().map_err(|_| format!("\u{201c}{s}\u{201d} is not a sample rate: type it in kHz, like 44.1"))?;
            if !v.is_finite() || v <= 0.0 {
                return Err("Type a sample rate in kHz, like 44.1".into());
            }
            Ok(if v >= 1000.0 { v.round() } else { (v * 1000.0).round() })
        }
        _ => {
            let v: u64 = s.parse().map_err(|_| format!("\u{201c}{s}\u{201d} is not a whole number"))?;
            if field == Field::Year && !(1000..=9999).contains(&v) {
                return Err("Type a year, like 1999".into());
            }
            Ok(v as f64)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Line(Line),
    Group { all: bool, items: Vec<Item> },
    /// What this build cannot read, kept as it was.
    Unknown(Node),
}

/// An order as chosen ("plays in the last N <unit>" keeps its N as typed).
#[derive(Clone, Debug, PartialEq)]
pub struct Order {
    pub key: SortKey,
    pub desc: bool,
    pub n: String,
    pub unit: Unit,
}

impl Order {
    fn of(s: Sort) -> Order {
        let (n, unit) = match s.key {
            SortKey::PlaysIn(n, u) => (n.to_string(), u),
            _ => ("30".into(), Unit::Days),
        };
        Order { key: s.key, desc: s.desc, n, unit }
    }

    fn sort(&self) -> Result<Sort, String> {
        let key = match self.key {
            SortKey::PlaysIn(..) => SortKey::PlaysIn(parse_count(&self.n).ok_or("Say how far back to count plays")?, self.unit),
            k => k,
        };
        Ok(Sort { key, desc: self.desc })
    }

    /// The direction's two ways, worded for its key.
    fn directions(&self) -> [&'static str; 2] {
        match self.key {
            SortKey::ArtistAlbum | SortKey::Title | SortKey::Artist | SortKey::Album => ["A to Z", "Z to A"],
            SortKey::Year => ["oldest first", "newest first"],
            SortKey::Added | SortKey::LastPlayed => ["longest ago first", "most recent first"],
            SortKey::Duration => ["shortest first", "longest first"],
            SortKey::Random => ["", ""],
            _ => ["least first", "most first"],
        }
    }
}

/// Where a line of the editor is in the draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum At {
    /// "Tracks that match all / any of these rules".
    Join,
    /// An item, by its place (a group's item: the group's place, then its own).
    Item(Vec<usize>),
    Sort,
    Limit,
}

/// One control as the editor draws it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Slot {
    pub kind: i32,
    pub role: i32,
    pub text: String,
    pub placeholder: String,
    pub label: String,
    pub width: f32,
    pub invalid: bool,
    pub on: bool,
}

/// One line as the editor draws it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub kind: i32,
    pub depth: i32,
    pub slots: Vec<Slot>,
    pub error: String,
    pub note: String,
    pub can_up: bool,
    pub can_down: bool,
    pub removable: bool,
    pub label: String,
}

/// What a choice offers: its entries, the one in effect, whether typing filters them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    pub title: String,
    pub labels: Vec<String>,
    pub checked: Option<usize>,
    pub filterable: bool,
}

/// What the editor picks values from: the library's names, and the playlists.
#[derive(Clone, Debug, Default)]
pub struct Names {
    pub artists: Vec<String>,
    pub album_artists: Vec<String>,
    pub genres: Vec<String>,
    pub labels: Vec<String>,
    pub formats: Vec<String>,
    /// (id, name, smart)
    pub playlists: Vec<(i64, String, bool)>,
    /// Playlists a rule here may not name (this one, and those that come back to it).
    pub excluded: Vec<i64>,
}

impl Names {
    pub fn playlist(&self, id: i64) -> Option<String> {
        self.playlists.iter().find(|p| p.0 == id).map(|p| p.1.clone())
    }

    fn values(&self, field: Field) -> &[String] {
        match field {
            Field::Artist => &self.artists,
            Field::AlbumArtist => &self.album_artists,
            Field::Genre => &self.genres,
            Field::Label => &self.labels,
            Field::Format => &self.formats,
            _ => &[],
        }
    }

    fn pickable(&self) -> Vec<&(i64, String, bool)> {
        self.playlists.iter().filter(|p| !self.excluded.contains(&p.0)).collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    /// The playlist being edited; None for a new one.
    pub id: Option<i64>,
    pub name: String,
    pub all: bool,
    pub items: Vec<Item>,
    pub order: Order,
    /// A stored order this build does not know, kept unless the user picks another.
    pub order_unknown: Option<Json>,
    pub limit_on: bool,
    pub limit_n: String,
    pub limit_unit: LimitUnit,
    pub by: Order,
    pub limit_unknown: Option<Json>,
    pub seed: i64,
    pub version: u32,
    pub extra: Map<String, Json>,
    /// Changed since it was opened.
    pub dirty: bool,
}

/// Characters a typed width allows for, in logical pixels per character (the body font).
const CH: f32 = 8.2;

impl Draft {
    /// A new smart playlist: one rule to fill in ("Genre is …").
    pub fn new() -> Draft {
        let mut d = Draft::from_smart(None, "", &Smart::default());
        d.items.push(Item::Line(Line::new(Field::Genre)));
        d
    }

    pub fn from_smart(id: Option<i64>, name: &str, s: &Smart) -> Draft {
        fn items(g: &Group) -> Vec<Item> {
            g.nodes
                .iter()
                .map(|n| match n {
                    Node::Cond(c) => Item::Line(Line::from_cond(c)),
                    Node::Group(g) => Item::Group { all: g.all, items: items(g) },
                    other => Item::Unknown(other.clone()),
                })
                .collect()
        }
        let (order, order_unknown) = match &s.sort {
            Sorting::Known(o) => (Order::of(*o), None),
            Sorting::Unknown(raw) => (Order::of(Sort::default()), Some(raw.clone())),
        };
        let (limit_on, limit_n, limit_unit, by, limit_unknown) = match &s.limit {
            Some(Limiting::Known(l)) => (true, l.n.to_string(), l.unit, Order::of(l.by), None),
            Some(Limiting::Unknown(raw)) => (false, "25".into(), LimitUnit::Tracks, Order::of(Sort::new(SortKey::Random)), Some(raw.clone())),
            None => (false, "25".into(), LimitUnit::Tracks, Order::of(Sort::new(SortKey::Random)), None),
        };
        Draft {
            id,
            name: name.to_owned(),
            all: s.root.all,
            items: items(&s.root),
            order,
            order_unknown,
            limit_on,
            limit_n,
            limit_unit,
            by,
            limit_unknown,
            seed: s.seed,
            version: s.version,
            extra: s.extra.clone(),
            dirty: false,
        }
    }

    /// A starting point: its rules and order, and its name when none is typed yet (or the
    /// name is another starting point's).
    pub fn apply_template(&mut self, i: usize, seed: i64) {
        let all = smart::templates();
        let Some((name, t)) = all.get(i) else { return };
        let keep_name = !self.name.trim().is_empty() && !all.iter().any(|(n, _)| *n == self.name.trim());
        let name = if keep_name { self.name.clone() } else { (*name).to_owned() };
        let mut d = Draft::from_smart(self.id, &name, t);
        d.seed = seed;
        d.dirty = true;
        *self = d;
    }

    /// The rule set, or the first line that does not say one yet (its place, and why).
    pub fn smart(&self) -> Result<Smart, (At, String)> {
        fn nodes(items: &[Item], path: &mut Vec<usize>) -> Result<Vec<Node>, (At, String)> {
            let mut out = Vec::new();
            for (i, it) in items.iter().enumerate() {
                path.push(i);
                let n = match it {
                    Item::Line(l) => Node::Cond(l.cond().map_err(|e| (At::Item(path.clone()), e))?),
                    Item::Group { all, items } => {
                        if items.is_empty() {
                            return Err((At::Item(path.clone()), "This group has no rules yet: add one, or remove the group".into()));
                        }
                        Node::Group(Group { all: *all, nodes: nodes(items, path)? })
                    }
                    Item::Unknown(n) => n.clone(),
                };
                path.pop();
                out.push(n);
            }
            Ok(out)
        }
        let root = Group { all: self.all, nodes: nodes(&self.items, &mut Vec::new())? };
        let sort = match &self.order_unknown {
            Some(raw) => Sorting::Unknown(raw.clone()),
            None => Sorting::Known(self.order.sort().map_err(|e| (At::Sort, e))?),
        };
        let limit = if let Some(raw) = &self.limit_unknown {
            Some(Limiting::Unknown(raw.clone()))
        } else if self.limit_on {
            let n = parse_count(&self.limit_n).ok_or((At::Limit, "Type how many".to_owned()))?;
            Some(Limiting::Known(Limit { n, unit: self.limit_unit, by: self.by.sort().map_err(|e| (At::Limit, e))? }))
        } else {
            None
        };
        Ok(Smart { version: self.version.max(smart::FORMAT), root, sort, limit, seed: self.seed, extra: self.extra.clone() })
    }

    fn item(&self, path: &[usize]) -> Option<&Item> {
        let (&first, rest) = path.split_first()?;
        let mut it = self.items.get(first)?;
        for &i in rest {
            match it {
                Item::Group { items, .. } => it = items.get(i)?,
                _ => return None,
            }
        }
        Some(it)
    }

    fn item_mut(&mut self, path: &[usize]) -> Option<&mut Item> {
        let (&first, rest) = path.split_first()?;
        let mut it = self.items.get_mut(first)?;
        for &i in rest {
            match it {
                Item::Group { items, .. } => it = items.get_mut(i)?,
                _ => return None,
            }
        }
        Some(it)
    }

    /// The list `path`'s item is in.
    fn siblings_mut(&mut self, path: &[usize]) -> Option<&mut Vec<Item>> {
        match path.split_last() {
            None => None,
            Some((_, [])) => Some(&mut self.items),
            Some((_, parent)) => match self.item_mut(parent)? {
                Item::Group { items, .. } => Some(items),
                _ => None,
            },
        }
    }

    fn line_mut(&mut self, at: &At) -> Option<&mut Line> {
        match at {
            At::Item(p) => match self.item_mut(p)? {
                Item::Line(l) => Some(l),
                _ => None,
            },
            _ => None,
        }
    }

    /// Add a rule: at the end of the group at `group` (None: the top), like the last rule
    /// there when there is one. Returns its place.
    pub fn add_rule(&mut self, group: Option<&[usize]>) -> Vec<usize> {
        self.dirty = true;
        let list = match group {
            None => &mut self.items,
            Some(p) => match self.item_mut(p) {
                Some(Item::Group { items, .. }) => items,
                _ => &mut self.items,
            },
        };
        let field = list.iter().rev().find_map(|i| if let Item::Line(l) = i { Some(l.field) } else { None }).unwrap_or(Field::Genre);
        list.push(Item::Line(Line::new(field)));
        let mut p = group.map(<[usize]>::to_vec).unwrap_or_default();
        p.push(list.len() - 1);
        p
    }

    /// Add a group (of one rule) at the top level: "any of these" inside "all of these", or
    /// the other way round.
    pub fn add_group(&mut self) -> Vec<usize> {
        self.dirty = true;
        self.items.push(Item::Group { all: !self.all, items: vec![Item::Line(Line::new(Field::Favourite))] });
        vec![self.items.len() - 1, 0]
    }

    pub fn remove(&mut self, path: &[usize]) {
        let Some(&last) = path.last() else { return };
        if let Some(list) = self.siblings_mut(path)
            && last < list.len()
        {
            list.remove(last);
            self.dirty = true;
        }
    }

    /// Move an item up (-1) or down (+1) among its neighbours. Returns its new place.
    pub fn move_item(&mut self, path: &[usize], dir: i32) -> Option<Vec<usize>> {
        let &last = path.last()?;
        let list = self.siblings_mut(path)?;
        let to = last as i64 + i64::from(dir);
        if to < 0 || to as usize >= list.len() {
            return None;
        }
        list.swap(last, to as usize);
        self.dirty = true;
        let mut p = path.to_vec();
        *p.last_mut()? = to as usize;
        Some(p)
    }

    /// Text typed into a control.
    pub fn type_text(&mut self, at: &At, role: Role, text: &str) {
        self.dirty = true;
        match (at, role) {
            (At::Sort, Role::SortN) => self.order.n = text.to_owned(),
            (At::Limit, Role::LimitN) => self.limit_n = text.to_owned(),
            (At::Limit, Role::ByN) => self.by.n = text.to_owned(),
            _ => {
                if let Some(l) = self.line_mut(at) {
                    l.touched = true;
                    match role {
                        Role::A => l.a = text.to_owned(),
                        Role::B => l.b = text.to_owned(),
                        Role::Within => l.within = text.to_owned(),
                        _ => {}
                    }
                }
            }
        }
    }

    /// Stars stepped up or down (a rating's value).
    pub fn step_stars(&mut self, at: &At, role: Role, delta: i32) {
        if let Some(l) = self.line_mut(at) {
            let s = if role == Role::B { &mut l.b } else { &mut l.a };
            let v = s.trim().parse::<i32>().unwrap_or(0);
            *s = (v + delta).clamp(0, 5).to_string();
            l.touched = true;
            self.dirty = true;
        }
    }

    /// The limit switched on or off.
    pub fn toggle_limit(&mut self) {
        self.dirty = true;
        if self.limit_unknown.take().is_some() {
            self.limit_on = true;
        } else {
            self.limit_on = !self.limit_on;
        }
    }

    /// What a choice offers.
    pub fn options(&self, at: &At, role: Role, names: &Names) -> Options {
        let list = |title: &str, labels: Vec<String>, checked: Option<usize>| Options { title: title.into(), labels, checked, filterable: false };
        let units = |n: &str, current: Unit| {
            let n = parse_count(n).unwrap_or(2);
            list("Unit", Unit::ALL.iter().map(|u| u.words(n).to_owned()).collect(), Unit::ALL.iter().position(|u| *u == current))
        };
        let sort_keys = |current: SortKey| {
            list(
                "Order",
                SortKey::CHOICES.iter().map(|k| k.label().to_owned()).collect(),
                SortKey::CHOICES.iter().position(|k| std::mem::discriminant(k) == std::mem::discriminant(&current)),
            )
        };
        match (at, role) {
            (At::Join, Role::Join) => list("Match", vec!["all".into(), "any".into()], Some(usize::from(!self.all))),
            (At::Item(p), Role::Join) => match self.item(p) {
                Some(Item::Group { all, .. }) => list("Match", vec!["all".into(), "any".into()], Some(usize::from(!*all))),
                _ => Options::default(),
            },
            (At::Sort, Role::SortKey) => sort_keys(self.order.key),
            (At::Sort, Role::SortDir) => list("Direction", self.order.directions().iter().map(|s| (*s).to_owned()).collect(), Some(usize::from(self.order.desc))),
            (At::Sort, Role::SortUnit) => units(&self.order.n, self.order.unit),
            (At::Limit, Role::LimitUnit) => {
                let n = parse_count(&self.limit_n).unwrap_or(2);
                list("Limit", LimitUnit::ALL.iter().map(|u| u.words(n).to_owned()).collect(), LimitUnit::ALL.iter().position(|u| *u == self.limit_unit))
            }
            (At::Limit, Role::ByKey) => sort_keys(self.by.key),
            (At::Limit, Role::ByDir) => list("Direction", self.by.directions().iter().map(|s| (*s).to_owned()).collect(), Some(usize::from(self.by.desc))),
            (At::Limit, Role::ByUnit) => units(&self.by.n, self.by.unit),
            (At::Item(p), role) => {
                let Some(Item::Line(l)) = self.item(p) else { return Options::default() };
                match role {
                    Role::Field => Options {
                        title: "Field".into(),
                        labels: Field::ALL.iter().map(|f| f.label().to_owned()).collect(),
                        checked: Field::ALL.iter().position(|f| *f == l.field),
                        filterable: true,
                    },
                    Role::Op => list("Condition", l.field.ops().iter().map(|o| l.field.op_label(*o).to_owned()).collect(), l.field.ops().iter().position(|o| *o == l.op)),
                    Role::Unit => units(&l.a, l.unit),
                    Role::WithinUnit => units(&l.within, l.within_unit),
                    Role::A if l.field == Field::Playlist => {
                        let p = names.pickable();
                        Options {
                            title: "Playlist".into(),
                            labels: p.iter().map(|p| if p.2 { format!("{} (smart)", p.1) } else { p.1.clone() }).collect(),
                            checked: p.iter().position(|p| p.0 == l.id),
                            filterable: true,
                        }
                    }
                    Role::A => {
                        let values = names.values(l.field);
                        Options { title: l.field.label().into(), labels: values.to_vec(), checked: values.iter().position(|v| *v == l.a), filterable: true }
                    }
                    _ => Options::default(),
                }
            }
            _ => Options::default(),
        }
    }

    /// Entry `i` of what `options` offered was chosen. `names` as given to `options`.
    pub fn choose(&mut self, at: &At, role: Role, i: usize, names: &Names) {
        self.dirty = true;
        let pick_sort = |o: &mut Order, i: usize| {
            if let Some(&k) = SortKey::CHOICES.get(i) {
                if std::mem::discriminant(&k) != std::mem::discriminant(&o.key) {
                    o.desc = k.default_desc();
                }
                o.key = match k {
                    SortKey::PlaysIn(..) => SortKey::PlaysIn(parse_count(&o.n).unwrap_or(30), o.unit),
                    k => k,
                };
            }
        };
        match (at, role) {
            (At::Join, Role::Join) => self.all = i == 0,
            (At::Item(p), Role::Join) => {
                if let Some(Item::Group { all, .. }) = self.item_mut(p) {
                    *all = i == 0;
                }
            }
            (At::Sort, Role::SortKey) => {
                self.order_unknown = None;
                pick_sort(&mut self.order, i);
            }
            (At::Sort, Role::SortDir) => self.order.desc = i == 1,
            (At::Sort, Role::SortUnit) => {
                if let Some(&u) = Unit::ALL.get(i) {
                    self.order.unit = u;
                }
            }
            (At::Limit, Role::LimitUnit) => {
                if let Some(&u) = LimitUnit::ALL.get(i) {
                    self.limit_unit = u;
                }
            }
            (At::Limit, Role::ByKey) => pick_sort(&mut self.by, i),
            (At::Limit, Role::ByDir) => self.by.desc = i == 1,
            (At::Limit, Role::ByUnit) => {
                if let Some(&u) = Unit::ALL.get(i) {
                    self.by.unit = u;
                }
            }
            (At::Item(_), role) => {
                let pickable: Vec<i64> = names.pickable().iter().map(|p| p.0).collect();
                let values: Vec<String> = self.line_mut(at).map(|l| names.values(l.field).to_vec()).unwrap_or_default();
                let Some(l) = self.line_mut(at) else { return };
                l.touched = true;
                match role {
                    Role::Field => {
                        if let Some(&f) = Field::ALL.get(i)
                            && f != l.field
                        {
                            let typed = |f: Field, op: Op| matches!(f.kind(), Kind::Text | Kind::Choice) && !(f.kind() == Kind::Choice && matches!(op, Op::Is | Op::IsNot));
                            let keep_text = typed(l.field, l.op) && f.ops().contains(&l.op) && typed(f, l.op);
                            let old_op = l.op;
                            let mut n = Line::new(f);
                            n.touched = true;
                            if keep_text {
                                n.op = old_op;
                                n.a = std::mem::take(&mut l.a);
                            }
                            *l = n;
                        }
                    }
                    Role::Op => {
                        if let Some(&op) = l.field.ops().get(i)
                            && op != l.op
                        {
                            let relative = |o: Op| matches!(o, Op::InLast | Op::NotInLast);
                            let picked = |o: Op| l.field.kind() == Kind::Choice && matches!(o, Op::Is | Op::IsNot);
                            // A value of another kind (a count for a date, a pick for typed text) starts again.
                            if (l.field.kind() == Kind::Date && relative(op) != relative(l.op)) || picked(op) != picked(l.op) {
                                l.a = default_value(l.field, op);
                                l.b.clear();
                            }
                            if op == Op::Between && l.b.is_empty() && l.field.kind() == Kind::Number {
                                l.b = l.a.clone();
                            }
                            l.op = op;
                        }
                    }
                    Role::Unit => {
                        if let Some(&u) = Unit::ALL.get(i) {
                            l.unit = u;
                        }
                    }
                    Role::WithinUnit => {
                        if let Some(&u) = Unit::ALL.get(i) {
                            l.within_unit = u;
                        }
                    }
                    Role::A if l.field == Field::Playlist => {
                        if let Some(&id) = pickable.get(i) {
                            l.id = id;
                        }
                    }
                    Role::A => {
                        if let Some(v) = values.get(i) {
                            l.a = v.clone();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The rules card's lines, with where each is in the draft.
    pub fn rule_rows(&self, names: &Names) -> Vec<(At, Row)> {
        let mut out = vec![(
            At::Join,
            Row {
                kind: JOIN,
                slots: vec![words("Tracks that match"), choice(Role::Join, if self.all { "all" } else { "any" }, "Match all or any of these rules", false), words("of these rules")],
                label: format!("Tracks that match {} of these rules", if self.all { "all" } else { "any" }),
                ..Row::default()
            },
        )];
        fn walk(items: &[Item], path: &mut Vec<usize>, depth: i32, names: &Names, out: &mut Vec<(At, Row)>, number: &mut usize) {
            let n = items.len();
            for (i, it) in items.iter().enumerate() {
                path.push(i);
                let mut row = match it {
                    Item::Line(l) => {
                        *number += 1;
                        line_row(l, names, *number)
                    }
                    Item::Group { all, items } => Row {
                        kind: GROUP,
                        slots: vec![words("Any track matching"), choice(Role::Join, if *all { "all" } else { "any" }, "Match all or any of this group's rules", false), words("of:")],
                        error: if items.is_empty() { "This group has no rules yet: add one, or remove the group".into() } else { String::new() },
                        label: format!("A group: tracks matching {} of its rules", if *all { "all" } else { "any" }),
                        ..Row::default()
                    },
                    Item::Unknown(node) => {
                        let why = match node {
                            Node::Unknown { why, .. } => why.clone(),
                            _ => String::new(),
                        };
                        Row {
                            kind: UNKNOWN,
                            note: format!("A rule from a newer version of Marquee Player ({why}). It matches nothing here and is kept as it is."),
                            label: "A rule this version cannot use".into(),
                            ..Row::default()
                        }
                    }
                };
                row.depth = depth;
                row.can_up = i > 0;
                row.can_down = i + 1 < n;
                row.removable = true;
                out.push((At::Item(path.clone()), row));
                if let Item::Group { items, .. } = it {
                    walk(items, path, depth + 1, names, out, number);
                }
                path.pop();
            }
        }
        walk(&self.items, &mut Vec::new(), 0, names, &mut out, &mut 0);
        out
    }

    /// The order and limit card's lines.
    pub fn tail_rows(&self) -> Vec<(At, Row)> {
        let order_slots = |o: &Order, key_role: Role, dir_role: Role, n_role: Role, unit_role: Role, what: &str| -> Vec<Slot> {
            let mut v = vec![choice(key_role, o.key.label(), &format!("{what}: order"), false)];
            if let SortKey::PlaysIn(..) = o.key {
                v.push(text(n_role, &o.n, "30", &format!("{what}: how far back"), 6, parse_count(&o.n).is_none()));
                v.push(choice(unit_role, o.unit.words(parse_count(&o.n).unwrap_or(2)), &format!("{what}: unit"), false));
            }
            if o.key != SortKey::Random {
                v.push(choice(dir_role, o.directions()[usize::from(o.desc)], &format!("{what}: direction"), false));
            }
            v
        };
        let mut sort = vec![words("Sort by")];
        sort.extend(order_slots(&self.order, Role::SortKey, Role::SortDir, Role::SortN, Role::SortUnit, "Sort"));
        let sort_row = Row {
            kind: SORT,
            slots: sort,
            note: if self.order_unknown.is_some() { "Sorted in a way from a newer version: shown by artist and album here. Choose an order to replace it.".into() } else { String::new() },
            error: self.order.sort().err().unwrap_or_default(),
            label: "Order".into(),
            ..Row::default()
        };
        let mut limit = vec![Slot { kind: SWITCH, role: Role::LimitOn.index(), on: self.limit_on, label: "Limit the playlist".into(), ..Slot::default() }, words("Limit to")];
        let n_bad = self.limit_on && parse_count(&self.limit_n).is_none();
        limit.push(text(Role::LimitN, &self.limit_n, "25", "Limit: how many", 6, n_bad));
        limit.push(choice(Role::LimitUnit, self.limit_unit.words(parse_count(&self.limit_n).unwrap_or(2)), "Limit: tracks, minutes or hours", false));
        limit.push(words("chosen by"));
        limit.extend(order_slots(&self.by, Role::ByKey, Role::ByDir, Role::ByN, Role::ByUnit, "Limit"));
        let limit_row = Row {
            kind: LIMIT,
            slots: limit,
            error: if n_bad { "Type how many: a whole number above 0".into() } else if self.limit_on { self.by.sort().err().unwrap_or_default() } else { String::new() },
            note: if self.limit_unknown.is_some() { "A limit from a newer version: nothing is shown rather than more than it allows. Switch it off or set one to replace it.".into() } else { String::new() },
            label: "Limit".into(),
            ..Row::default()
        };
        vec![(At::Sort, sort_row), (At::Limit, limit_row)]
    }
}

fn words(t: &str) -> Slot {
    Slot { kind: WORDS, role: Role::Words.index(), text: t.into(), ..Slot::default() }
}

fn choice(role: Role, t: &str, label: &str, invalid: bool) -> Slot {
    Slot { kind: CHOICE, role: role.index(), text: t.into(), label: label.into(), invalid, ..Slot::default() }
}

fn text(role: Role, t: &str, placeholder: &str, label: &str, chars: usize, invalid: bool) -> Slot {
    Slot { kind: TEXT, role: role.index(), text: t.into(), placeholder: placeholder.into(), label: label.into(), width: 24.0 + CH * chars as f32, invalid, ..Slot::default() }
}

/// A rule's line: field, operator, value(s), with the words between.
fn line_row(l: &Line, names: &Names, number: usize) -> Row {
    let what = format!("Rule {number}");
    let err = if l.touched || !l.a.is_empty() { l.cond().err() } else { None };
    let bad = err.is_some();
    let mut slots = vec![choice(Role::Field, l.field.label(), &format!("{what}: field"), false)];
    let value_text = |role: Role, s: &str, chars: usize| -> Slot {
        let placeholder = match l.field {
            Field::Duration => "3:30",
            Field::SampleRate => "44.1",
            Field::Year => "1999",
            _ if l.field.kind() == Kind::Date => "2024-03-15",
            _ => "",
        };
        text(role, s, placeholder, &format!("{what}: value"), chars, bad)
    };
    if l.field == Field::PlaysIn {
        slots.push(text(Role::Within, &l.within, "30", &format!("{what}: how far back"), 6, parse_count(&l.within).is_none()));
        slots.push(choice(Role::WithinUnit, l.within_unit.words(parse_count(&l.within).unwrap_or(2)), &format!("{what}: unit"), false));
    }
    slots.push(choice(Role::Op, l.field.op_label(l.op), &format!("{what}: condition"), false));
    match (l.field.kind(), l.op) {
        (Kind::Bool, _) => {}
        (Kind::Choice, Op::Is | Op::IsNot) => {
            let t = if l.a.trim().is_empty() { "Choose\u{2026}".to_owned() } else { l.a.clone() };
            slots.push(choice(Role::A, &t, &format!("{what}: value"), bad));
        }
        (Kind::Text | Kind::Choice, _) => slots.push(text(Role::A, &l.a, "text", &format!("{what}: text"), 22, bad)),
        (Kind::Playlist, _) => {
            let t = names.playlist(l.id).unwrap_or_else(|| if l.id > 0 { "a missing playlist".into() } else { "Choose\u{2026}".into() });
            slots.push(choice(Role::A, &t, &format!("{what}: playlist"), bad));
        }
        (Kind::Number, op) if l.field == Field::Rating => {
            let stars = |s: &str| {
                let n = s.trim().parse::<u8>().unwrap_or(0);
                if n == 0 { "not rated".to_owned() } else { crate::rows::stars(n) }
            };
            slots.push(Slot { kind: STARS, role: Role::A.index(), text: stars(&l.a), label: format!("{what}: stars"), invalid: bad, ..Slot::default() });
            if op == Op::Between {
                slots.push(words("and"));
                slots.push(Slot { kind: STARS, role: Role::B.index(), text: stars(&l.b), label: format!("{what}: stars, up to"), invalid: bad, ..Slot::default() });
            }
        }
        (Kind::Number, op) => {
            let chars = match l.field {
                Field::Year | Field::Duration => 7,
                _ => 6,
            };
            slots.push(value_text(Role::A, &l.a, chars));
            if op == Op::Between {
                slots.push(words("and"));
                slots.push(value_text(Role::B, &l.b, chars));
            }
            match l.field {
                Field::Bitrate => slots.push(words("kbps")),
                Field::SampleRate => slots.push(words("kHz")),
                Field::BitDepth => slots.push(words("bits")),
                Field::Plays | Field::PlaysIn => slots.push(words(if l.a.trim() == "1" && op != Op::Between { "play" } else { "plays" })),
                _ => {}
            }
        }
        (Kind::Date, Op::InLast | Op::NotInLast) => {
            slots.push(value_text(Role::A, &l.a, 5));
            slots.push(choice(Role::Unit, l.unit.words(parse_count(&l.a).unwrap_or(2)), &format!("{what}: unit"), false));
        }
        (Kind::Date, op) => {
            slots.push(value_text(Role::A, &l.a, 11));
            if op == Op::Between {
                slots.push(words("and"));
                slots.push(value_text(Role::B, &l.b, 11));
            }
        }
    }
    let label = match l.cond() {
        Ok(c) => format!("{what}: {}", smart::cond_words(&c, &|id| names.playlist(id))),
        Err(_) => format!("{what}: {} {}", l.field.label(), l.field.op_label(l.op)),
    };
    Row { kind: RULE, slots, error: err.unwrap_or_default(), label, ..Row::default() }
}

/// Entries of a list that match what is typed in its filter: every word somewhere in the
/// entry, folded as search folds (case, accents, kana, width). Their indices.
pub fn filter(labels: &[String], typed: &str) -> Vec<usize> {
    let folded = library::fold::fold(typed);
    let words: Vec<&str> = folded.split_whitespace().collect();
    if words.is_empty() {
        return (0..labels.len()).collect();
    }
    labels.iter().enumerate().filter(|(_, l)| {
        let f = library::fold::fold(l);
        words.iter().all(|w| f.contains(w))
    }).map(|(i, _)| i).collect()
}

/// A name no other playlist has: `base`, else "base 2", "base 3"...
pub fn unique_name(base: &str, taken: &[String]) -> String {
    let same = crate::playlists::newplaylist::same_name;
    if !taken.iter().any(|t| same(t, base)) {
        return base.to_owned();
    }
    (2..).map(|n| format!("{base} {n}")).find(|c| !taken.iter().any(|t| same(t, c))).unwrap_or_else(|| base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        Names {
            artists: vec!["Beyoncé".into(), "Miles Davis".into(), "ヨルシカ".into()],
            album_artists: vec!["Miles Davis".into()],
            genres: vec!["Jazz".into(), "Rock".into()],
            labels: vec!["Blue Note".into()],
            formats: vec!["FLAC".into(), "MP3".into()],
            playlists: vec![(1, "Road trip".into(), false), (2, "Top".into(), true), (3, "This one".into(), true)],
            excluded: vec![3],
        }
    }

    fn texts(row: &Row) -> Vec<String> {
        row.slots.iter().map(|s| s.text.clone()).collect()
    }

    #[test]
    fn a_rule_reads_as_a_sentence_of_controls() {
        let n = names();
        let mut d = Draft::new();
        let rows = d.rule_rows(&n);
        assert_eq!(texts(&rows[0].1), ["Tracks that match", "all", "of these rules"]);
        assert_eq!(texts(&rows[1].1), ["Genre", "is", "Choose\u{2026}"]);
        assert_eq!(rows[1].1.error, "", "a new line is not an error yet");
        assert!(d.smart().is_err(), "but it cannot be saved");
        // Pick the genre from the library's.
        let at = rows[1].0.clone();
        let o = d.options(&at, Role::A, &n);
        assert_eq!((o.labels.clone(), o.filterable), (vec!["Jazz".to_owned(), "Rock".into()], true));
        d.choose(&at, Role::A, 0, &n);
        assert_eq!(texts(&d.rule_rows(&n)[1].1), ["Genre", "is", "Jazz"]);
        assert_eq!(d.rule_rows(&n)[1].1.label, "Rule 1: Genre is Jazz");
        // Then "contains" takes typed text.
        let ops = d.options(&at, Role::Op, &n);
        d.choose(&at, Role::Op, ops.labels.iter().position(|l| l == "contains").unwrap(), &n);
        let row = &d.rule_rows(&n)[1].1;
        assert_eq!(row.slots[2].kind, TEXT);
        assert_eq!(row.error, "Type some text");
        d.type_text(&at, Role::A, "fusion");
        assert_eq!(d.smart().unwrap().to_json(), r#"{"match":"all","rules":[{"field":"genre","op":"contains","value":"fusion"}],"seed":1,"sort":{"by":"artist_album","desc":false},"v":1}"#);
    }

    #[test]
    fn each_kind_of_field_has_its_own_controls() {
        let n = names();
        let row = |l: Line| texts(&line_row(&l, &n, 1));
        let kinds = |l: Line| line_row(&l, &n, 1).slots.iter().map(|s| s.kind).collect::<Vec<_>>();
        assert_eq!(row(Line::new(Field::Rating)), ["Rating", "is at least", "★★★★☆"]);
        assert_eq!(kinds(Line::new(Field::Rating)), [CHOICE, CHOICE, STARS]);
        assert_eq!(row(Line::new(Field::Favourite)), ["Favourite", "yes"]);
        assert_eq!(row(Line::new(Field::Played)), ["Played", "never"]);
        assert_eq!(row(Line::new(Field::Duration)), ["Length", "is at least", "5:00"]);
        assert_eq!(row(Line::new(Field::Bitrate)), ["Bitrate", "is at least", "320", "kbps"]);
        assert_eq!(row(Line::new(Field::LastPlayed)), ["Last played", "is in the last", "1", "month"]);
        assert_eq!(row(Line::new(Field::PlaysIn)), ["Plays in the last", "30", "days", "is at least", "1", "play"]);
        assert_eq!(row(Line { op: Op::Between, b: "2000".into(), a: "1990".into(), ..Line::new(Field::Year) }), ["Year", "is between", "1990", "and", "2000"]);
        assert_eq!(row(Line { op: Op::Before, a: "2024-03".into(), ..Line::new(Field::Added) }), ["Date added", "is before", "2024-03"]);
        assert_eq!(row(Line { id: 2, ..Line::new(Field::Playlist) }), ["Playlist", "is in", "Top"]);
        assert_eq!(row(Line::new(Field::Title)), ["Title", "is", ""]);
        assert_eq!(kinds(Line::new(Field::Title))[2], TEXT, "typed, not picked");
    }

    #[test]
    fn typed_values_are_read_forgivingly_or_explained() {
        let p = |f, s| parse_number(f, s);
        assert_eq!(p(Field::Duration, "3:30"), Ok(210.0));
        assert_eq!(p(Field::Duration, "5"), Ok(300.0));
        assert_eq!(p(Field::Duration, "1:02:03"), Ok(3723.0));
        assert!(p(Field::Duration, "3:75").unwrap_err().contains("not a length"));
        assert_eq!(p(Field::SampleRate, "44.1"), Ok(44_100.0));
        assert_eq!(p(Field::SampleRate, "96 kHz"), Ok(96_000.0));
        assert_eq!(p(Field::SampleRate, "48000"), Ok(48_000.0));
        assert_eq!(p(Field::SampleRate, "44,1"), Ok(44_100.0));
        assert_eq!(p(Field::Year, "1999"), Ok(1999.0));
        assert_eq!(p(Field::Year, "99"), Err("Type a year, like 1999".into()));
        assert_eq!(p(Field::Plays, " 12 "), Ok(12.0));
        assert!(p(Field::Plays, "1.5").unwrap_err().contains("whole number"));
        assert_eq!(p(Field::Plays, ""), Err("Type a number".into()));
        // Dates.
        let mut l = Line { op: Op::After, a: "2024/3/5".into(), ..Line::new(Field::Added) };
        assert_eq!(l.cond().unwrap().value, Value::Date(DateSpec::parse("2024-03-05").unwrap()));
        l.a = "next tuesday".into();
        assert!(l.cond().unwrap_err().contains("is not a date"));
        // Values shown back as typed form.
        let c = Cond::new(Field::Duration, Op::AtLeast, Value::Num(330.0));
        assert_eq!(Line::from_cond(&c).a, "5:30");
        assert_eq!(Line::from_cond(&Cond::new(Field::SampleRate, Op::Is, Value::Num(88_200.0))).a, "88.2");
    }

    #[test]
    fn changing_the_field_or_operator_keeps_what_still_makes_sense() {
        let n = names();
        let mut d = Draft::new();
        let at = At::Item(vec![0]);
        let field = |d: &Draft, f: Field| d.options(&at, Role::Field, &n).labels.iter().position(|l| l == f.label()).unwrap();
        // Title contains "love" → Album: the typed text stays.
        d.choose(&at, Role::Field, field(&d, Field::Title), &n);
        let contains = d.options(&at, Role::Op, &n).labels.iter().position(|l| l == "contains").unwrap();
        d.choose(&at, Role::Op, contains, &n);
        d.type_text(&at, Role::A, "love");
        d.choose(&at, Role::Field, field(&d, Field::Album), &n);
        assert_eq!(texts(&d.rule_rows(&n)[1].1), ["Album", "contains", "love"]);
        // → Rating: its own operator and value.
        d.choose(&at, Role::Field, field(&d, Field::Rating), &n);
        assert_eq!(texts(&d.rule_rows(&n)[1].1), ["Rating", "is at least", "★★★★☆"]);
        d.step_stars(&at, Role::A, 1);
        d.step_stars(&at, Role::A, 1);
        assert_eq!(texts(&d.rule_rows(&n)[1].1)[2], "★★★★★", "no more than 5");
        d.step_stars(&at, Role::A, -9);
        assert_eq!(texts(&d.rule_rows(&n)[1].1)[2], "not rated");
        // → Last played; "in the last" to "before" starts the value again.
        d.choose(&at, Role::Field, field(&d, Field::LastPlayed), &n);
        d.type_text(&at, Role::A, "6");
        d.choose(&at, Role::Unit, 2, &n);
        assert_eq!(texts(&d.rule_rows(&n)[1].1), ["Last played", "is in the last", "6", "months"]);
        let before = d.options(&at, Role::Op, &n).labels.iter().position(|l| l == "is before").unwrap();
        d.choose(&at, Role::Op, before, &n);
        assert_eq!(texts(&d.rule_rows(&n)[1].1), ["Last played", "is before", ""]);
        // A number "between" starts from the first value.
        d.choose(&at, Role::Field, field(&d, Field::Year), &n);
        d.type_text(&at, Role::A, "1990");
        let between = d.options(&at, Role::Op, &n).labels.iter().position(|l| l == "is between").unwrap();
        d.choose(&at, Role::Op, between, &n);
        assert_eq!(texts(&d.rule_rows(&n)[1].1), ["Year", "is between", "1990", "and", "1990"]);
    }

    #[test]
    fn rules_are_added_grouped_moved_and_removed() {
        let n = names();
        let mut d = Draft::new();
        d.choose(&At::Item(vec![0]), Role::A, 0, &n);
        assert_eq!(d.add_rule(None), vec![1], "like the last: another genre");
        assert_eq!(texts(&d.rule_rows(&n)[2].1)[0], "Genre");
        let g = d.add_group();
        assert_eq!(g, vec![2, 0]);
        assert_eq!(d.add_rule(Some(&[2])), vec![2, 1]);
        let rows = d.rule_rows(&n);
        assert_eq!(rows.iter().map(|r| (r.1.kind, r.1.depth)).collect::<Vec<_>>(), vec![(JOIN, 0), (RULE, 0), (RULE, 0), (GROUP, 0), (RULE, 1), (RULE, 1)]);
        assert_eq!(texts(&rows[3].1), ["Any track matching", "any", "of:"], "the other way from the top");
        assert_eq!((rows[1].1.can_up, rows[1].1.can_down, rows[3].1.can_down), (false, true, false));
        // Move the group up; remove a rule in it; an empty group says so.
        assert_eq!(d.move_item(&[2], -1), Some(vec![1]));
        assert_eq!(d.rule_rows(&n)[2].1.kind, GROUP);
        assert_eq!(d.move_item(&[0], -1), None, "nothing above the first");
        d.remove(&[1, 1]);
        d.remove(&[1, 0]);
        assert_eq!(d.rule_rows(&n)[2].1.error, "This group has no rules yet: add one, or remove the group");
        assert!(matches!(d.smart(), Err((At::Item(p), _)) if p == vec![1]));
        d.remove(&[1]);
        assert!(d.dirty);
        // The second genre is not chosen yet: that is what stops a save.
        assert!(matches!(d.smart(), Err((At::Item(p), _)) if p == vec![1]));
        d.remove(&[1]);
        assert!(d.smart().is_ok());
    }

    #[test]
    fn order_and_limit_read_as_a_sentence_too() {
        let n = names();
        let mut d = Draft::new();
        let rows = d.tail_rows();
        assert_eq!(texts(&rows[0].1), ["Sort by", "Artist and album", "A to Z"]);
        assert_eq!(texts(&rows[1].1), ["", "Limit to", "25", "tracks", "chosen by", "Random"]);
        assert!(!rows[1].1.slots[0].on);
        d.toggle_limit();
        d.type_text(&At::Limit, Role::LimitN, "2");
        d.choose(&At::Limit, Role::LimitUnit, 2, &n);
        let key = SortKey::CHOICES.iter().position(|k| *k == SortKey::Plays).unwrap();
        d.choose(&At::Limit, Role::ByKey, key, &n);
        assert_eq!(texts(&d.tail_rows()[1].1), ["", "Limit to", "2", "hours", "chosen by", "Play count", "most first"]);
        let plays_in = SortKey::CHOICES.iter().position(|k| matches!(k, SortKey::PlaysIn(..))).unwrap();
        d.choose(&At::Sort, Role::SortKey, plays_in, &n);
        d.type_text(&At::Sort, Role::SortN, "12");
        d.choose(&At::Sort, Role::SortUnit, 2, &n);
        assert_eq!(texts(&d.tail_rows()[0].1), ["Sort by", "Plays in the last", "12", "months", "most first"]);
        d.choose(&At::Item(vec![0]), Role::A, 1, &n);
        let s = d.smart().unwrap();
        assert_eq!(s.order(), Sort { key: SortKey::PlaysIn(12, Unit::Months), desc: true });
        assert_eq!(s.limit, Some(Limiting::Known(Limit { n: 2, unit: LimitUnit::Hours, by: Sort::new(SortKey::Plays) })));
        assert_eq!(smart::summary(&s, &|_| None), "Genre is Rock \u{b7} 2 hours, most played");
        d.type_text(&At::Limit, Role::LimitN, "0");
        assert_eq!(d.tail_rows()[1].1.error, "Type how many: a whole number above 0");
        assert!(matches!(d.smart(), Err((At::Limit, _))));
    }

    #[test]
    fn a_playlist_cannot_name_itself_or_one_that_names_it() {
        let n = names();
        let mut d = Draft::new();
        let at = At::Item(vec![0]);
        let f = d.options(&at, Role::Field, &n).labels.iter().position(|l| l == "Playlist").unwrap();
        d.choose(&at, Role::Field, f, &n);
        assert_eq!(d.options(&at, Role::A, &n).labels, ["Road trip", "Top (smart)"]);
        d.choose(&at, Role::A, 1, &n);
        assert_eq!(texts(&d.rule_rows(&n)[1].1), ["Playlist", "is in", "Top"]);
        assert_eq!(d.smart().unwrap().root.nodes, vec![Node::Cond(Cond::new(Field::Playlist, Op::In, Value::Id(2)))]);
    }

    #[test]
    fn stored_rules_come_back_as_they_were_and_unknown_ones_stay() {
        let n = names();
        for (name, t) in smart::templates() {
            let d = Draft::from_smart(Some(5), name, &t);
            assert_eq!(d.smart().unwrap(), t, "{name}");
        }
        let newer = Smart::from_json(r#"{"v":2,"rules":[{"field":"mood","op":"is","value":"calm"},{"field":"favourite","op":"yes"}],"sort":{"by":"energy"},"limit":{"n":3,"unit":"albums"},"x":1}"#);
        let d = Draft::from_smart(Some(9), "Calm", &newer);
        let rows = d.rule_rows(&n);
        assert_eq!(rows[1].1.kind, UNKNOWN);
        assert!(rows[1].1.note.contains("the field “mood”"));
        assert!(d.tail_rows()[0].1.note.contains("newer version"));
        assert!(d.tail_rows()[1].1.note.contains("newer version"));
        // Saved untouched, it is what it was.
        assert_eq!(d.smart().unwrap(), newer);
        // Removing the unknown rule and setting an order replaces just those.
        let mut d2 = d.clone();
        d2.remove(&[0]);
        d2.choose(&At::Sort, Role::SortKey, 1, &n);
        let s = d2.smart().unwrap();
        assert_eq!(s.root.nodes.len(), 1);
        assert_eq!(s.order(), Sort::new(SortKey::Title));
        assert!(matches!(s.limit, Some(Limiting::Unknown(_))), "the limit is still the newer one's");
        assert_eq!(s.version, 2);
        assert_eq!(s.extra.get("x"), Some(&serde_json::json!(1)));
    }

    #[test]
    fn templates_fill_the_draft_and_name_it() {
        let mut d = Draft::new();
        d.apply_template(2, 77);
        assert_eq!(d.name, "Top rated");
        assert_eq!(d.seed, 77);
        assert!(d.smart().is_ok());
        d.name = "Mine".into();
        d.apply_template(4, 1);
        assert_eq!(d.name, "Mine", "a typed name stays");
        d.name = "Top rated".into();
        d.apply_template(4, 1);
        assert_eq!(d.name, "Lossless only", "another starting point's name follows");
    }

    #[test]
    fn lists_filter_as_search_does() {
        let labels: Vec<String> = ["Beyoncé", "Miles Davis", "ヨルシカ", "Davis Sisters"].map(String::from).to_vec();
        assert_eq!(filter(&labels, ""), [0, 1, 2, 3]);
        assert_eq!(filter(&labels, "beyonce"), [0]);
        assert_eq!(filter(&labels, "DAVIS"), [1, 3]);
        assert_eq!(filter(&labels, "davis mil"), [1]);
        assert_eq!(filter(&labels, "よるしか"), [2]);
        assert!(filter(&labels, "zz").is_empty());
    }

    #[test]
    fn names_are_made_unique() {
        let taken: Vec<String> = ["Jazz".into(), "Jazz 2".into(), "Road  Trip".into()].to_vec();
        assert_eq!(unique_name("Jazz", &taken), "Jazz 3");
        assert_eq!(unique_name("road trip", &taken), "road trip 2");
        assert_eq!(unique_name("Soul", &taken), "Soul");
    }

    #[test]
    fn roles_round_trip() {
        for r in Role::ALL {
            assert_eq!(Role::from_index(r.index()), Some(r));
        }
        assert_eq!(Role::from_index(-1), None);
        assert_eq!(Role::from_index(99), None);
    }
}
