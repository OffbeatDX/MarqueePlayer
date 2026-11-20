//! Calendar arithmetic for smart playlist rules: dates as typed ("2024", "2024-03",
//! "2024-03-15"), and "in the last N days / weeks / months / years". Everything takes the
//! time it is now and the local time zone as arguments (`Ctx`), so no rule depends on the
//! clock of the machine that evaluates it and tests can fix both.
//!
//! The zone gives the offset from UTC at each instant (`Zone::Rules`: the system's rules,
//! daylight saving included), not one offset for all time: a day that starts on the other side
//! of a daylight-saving change from now starts at its own local midnight, not an hour off.

/// The local time zone: its offset from UTC (seconds east) at a Unix time.
#[derive(Clone, Copy, Debug)]
pub enum Zone {
    /// One offset at every instant (tests; UTC).
    Fixed(i64),
    /// The offset at each instant, by these rules (the system's, `set_local_zone`).
    Rules(fn(i64) -> i64),
}

impl Zone {
    /// The offset from UTC at Unix time `t`.
    pub fn offset_at(self, t: i64) -> i64 {
        match self {
            Zone::Fixed(o) => o,
            Zone::Rules(f) => f(t),
        }
    }
}

/// When a rule is evaluated: now (Unix seconds) and the local time zone, for dates typed in
/// local time and for counting back on the calendar.
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    pub now: i64,
    pub zone: Zone,
}

/// The local zone `Ctx::now` uses (the app gives the system's: `set_local_zone`).
static LOCAL_ZONE: std::sync::OnceLock<fn(i64) -> i64> = std::sync::OnceLock::new();

/// The system's local time zone, as the offset from UTC in seconds east at a Unix time (the
/// app reads it from the system, which the library does not ask). Set once; later calls are
/// ignored.
pub fn set_local_zone(offset_at: fn(i64) -> i64) {
    let _ = LOCAL_ZONE.set(offset_at);
}

impl Ctx {
    /// At `now`, in a zone of one fixed offset `tz` (seconds east).
    pub fn new(now: i64, tz: i64) -> Ctx {
        Ctx { now, zone: Zone::Fixed(tz) }
    }

    pub fn with_zone(now: i64, zone: Zone) -> Ctx {
        Ctx { now, zone }
    }

    /// Now, in the local zone given by `set_local_zone` (UTC before it is).
    pub fn now() -> Ctx {
        let zone = LOCAL_ZONE.get().map_or(Zone::Fixed(0), |f| Zone::Rules(*f));
        Ctx::with_zone(crate::db::now(), zone)
    }

    /// The offset from UTC at Unix time `t`.
    pub fn offset_at(&self, t: i64) -> i64 {
        self.zone.offset_at(t)
    }

    /// Unix time `t` as local seconds since 1970-01-01 00:00 local.
    pub fn local(&self, t: i64) -> i64 {
        t + self.offset_at(t)
    }

    /// The first Unix time whose local time is `local` or later: the instant itself; the
    /// earlier of two when the clock went back over it; the end of the gap when the clock
    /// jumped over it. (Assumes at most one change of offset within a day of it, as in every
    /// real zone.)
    pub fn instant(&self, local: i64) -> i64 {
        let (before, after) = (self.offset_at(local - 86_400 - 50_400), self.offset_at(local + 86_400 + 50_400));
        let fits = |o: i64| self.offset_at(local - o) == o;
        match (fits(before), fits(after)) {
            (true, true) => (local - before).min(local - after),
            (true, false) => local - before,
            (false, true) => local - after,
            (false, false) => {
                // In the gap: the first instant past it, by halving between the two guesses.
                let (mut lo, mut hi) = ((local - before).min(local - after), (local - before).max(local - after));
                while hi - lo > 1 {
                    let mid = lo + (hi - lo) / 2;
                    if self.local(mid) >= local { hi = mid } else { lo = mid }
                }
                hi
            }
        }
    }

    /// Unix seconds at the local midnight that starts day `days` (days since 1970-01-01,
    /// local calendar): in its own offset, which may not be today's.
    pub fn day_start(&self, days: i64) -> i64 {
        self.instant(days * 86_400)
    }

    /// The local calendar day (days since 1970-01-01) of Unix time `t`.
    pub fn day_of(&self, t: i64) -> i64 {
        self.local(t).div_euclid(86_400)
    }
}

/// A span of time counted back from now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Days,
    Weeks,
    Months,
    Years,
}

impl Unit {
    pub const ALL: [Unit; 4] = [Unit::Days, Unit::Weeks, Unit::Months, Unit::Years];

    pub fn key(self) -> &'static str {
        match self {
            Unit::Days => "days",
            Unit::Weeks => "weeks",
            Unit::Months => "months",
            Unit::Years => "years",
        }
    }

    pub fn from_key(s: &str) -> Option<Unit> {
        Unit::ALL.into_iter().find(|u| u.key() == s)
    }

    /// "days", or "day" for one.
    pub fn words(self, n: u32) -> &'static str {
        match (self, n == 1) {
            (Unit::Days, true) => "day",
            (Unit::Days, false) => "days",
            (Unit::Weeks, true) => "week",
            (Unit::Weeks, false) => "weeks",
            (Unit::Months, true) => "month",
            (Unit::Months, false) => "months",
            (Unit::Years, true) => "year",
            (Unit::Years, false) => "years",
        }
    }
}

/// Unix seconds `n` units before `ctx.now`: back on the local calendar to the same local time
/// of day (the 31st of a shorter month is its last day), as "the last 6 months" is meant, and
/// "the last 7 days" too: across a daylight-saving change that is an hour more or less than 7
/// times 24 hours.
pub fn ago(ctx: Ctx, n: u32, unit: Unit) -> i64 {
    let n = i64::from(n);
    let local = ctx.local(ctx.now);
    let (days, secs) = (local.div_euclid(86_400), local.rem_euclid(86_400));
    let day = match unit {
        Unit::Days => days - n,
        Unit::Weeks => days - n * 7,
        Unit::Months | Unit::Years => {
            let months = if unit == Unit::Years { n * 12 } else { n };
            let (y, m, d) = civil_from_days(days);
            let total = y * 12 + i64::from(m) - 1 - months;
            let (y2, m2) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
            days_from_civil(y2, m2, d.min(days_in_month(y2, m2)))
        }
    };
    ctx.instant(day * 86_400 + secs)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// A date as typed in a rule: a year, a month of a year, or a day. A year or a month stands
/// for all of it: "after 2024" is after the end of 2024, "before 2024" before its start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DateSpec {
    pub y: i32,
    pub m: Option<u8>,
    pub d: Option<u8>,
}

impl DateSpec {
    /// Read a date the forgiving way the tag editor does: "2024-03-15", "2024/3/15",
    /// "2024.03.15", "20240315", "2024-03", "2024". None when it is not a real date (or the
    /// year is outside 1000..=9999).
    pub fn parse(s: &str) -> Option<DateSpec> {
        let s = s.trim();
        let parts: Vec<&str> = if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
            vec![&s[..4], &s[4..6], &s[6..]]
        } else {
            s.split(['-', '/', '.']).collect()
        };
        if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) || p.len() > 4) {
            return None;
        }
        if parts[0].len() != 4 {
            return None;
        }
        let y: i32 = parts[0].parse().ok()?;
        if !(1000..=9999).contains(&y) {
            return None;
        }
        let m: Option<u8> = match parts.get(1) {
            Some(p) if p.len() <= 2 => Some(p.parse().ok().filter(|m| (1..=12).contains(m))?),
            Some(_) => return None,
            None => None,
        };
        let d: Option<u8> = match parts.get(2) {
            Some(p) if p.len() <= 2 => {
                let d: u8 = p.parse().ok()?;
                let max = days_in_month(i64::from(y), u32::from(m?)) as u8;
                if !(1..=max).contains(&d) {
                    return None;
                }
                Some(d)
            }
            Some(_) => return None,
            None => None,
        };
        Some(DateSpec { y, m, d })
    }

    /// "2024", "2024-03", "2024-03-15".
    pub fn iso(&self) -> String {
        match (self.m, self.d) {
            (Some(m), Some(d)) => format!("{:04}-{m:02}-{d:02}", self.y),
            (Some(m), None) => format!("{:04}-{m:02}", self.y),
            _ => format!("{:04}", self.y),
        }
    }

    /// The first day it covers (days since 1970-01-01).
    pub fn first_day(&self) -> i64 {
        days_from_civil(i64::from(self.y), u32::from(self.m.unwrap_or(1)), u32::from(self.d.unwrap_or(1)))
    }

    /// The day after the last day it covers.
    pub fn end_day(&self) -> i64 {
        match (self.m, self.d) {
            (Some(_), Some(_)) => self.first_day() + 1,
            (Some(m), None) => self.first_day() + i64::from(days_in_month(i64::from(self.y), u32::from(m))),
            _ => days_from_civil(i64::from(self.y) + 1, 1, 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip() {
        for days in [-719_468, -1, 0, 1, 10_957, 19_723, 20_407, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
    }

    #[test]
    fn dates_are_read_forgivingly_and_only_when_real() {
        let d = |s| DateSpec::parse(s).map(|d| d.iso());
        assert_eq!(d("2024-03-15"), Some("2024-03-15".into()));
        assert_eq!(d("2024/3/5"), Some("2024-03-05".into()));
        assert_eq!(d("2024.03.15"), Some("2024-03-15".into()));
        assert_eq!(d("20240315"), Some("2024-03-15".into()));
        assert_eq!(d(" 2024-03 "), Some("2024-03".into()));
        assert_eq!(d("2024"), Some("2024".into()));
        assert_eq!(d("2024-02-29"), Some("2024-02-29".into()));
        for bad in ["", "2023-02-29", "2024-13", "2024-00", "2024-01-32", "24-01-01", "abcd", "2024-1-1-1", "0999", "2024--01", "2024-03-15x", "１９９９"] {
            assert_eq!(DateSpec::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_year_or_a_month_covers_all_of_it() {
        let y = DateSpec::parse("2024").unwrap();
        assert_eq!((y.first_day(), y.end_day()), (days_from_civil(2024, 1, 1), days_from_civil(2025, 1, 1)));
        let m = DateSpec::parse("2024-02").unwrap();
        assert_eq!(m.end_day() - m.first_day(), 29);
        let d = DateSpec::parse("2024-12-31").unwrap();
        assert_eq!(d.end_day(), days_from_civil(2025, 1, 1));
    }

    #[test]
    fn months_and_years_go_back_on_the_local_calendar() {
        // 2024-03-31 12:00 local, at UTC+2.
        let tz = 7200;
        let now = days_from_civil(2024, 3, 31) * 86_400 + 12 * 3600 - tz;
        let ctx = Ctx::new(now, tz);
        let local = |t: i64| {
            let l = t + tz;
            (civil_from_days(l.div_euclid(86_400)), l.rem_euclid(86_400))
        };
        assert_eq!(local(ago(ctx, 1, Unit::Months)), ((2024, 2, 29), 12 * 3600), "the 31st of a shorter month is its last day");
        assert_eq!(local(ago(ctx, 6, Unit::Months)), ((2023, 9, 30), 12 * 3600));
        assert_eq!(local(ago(ctx, 13, Unit::Months)), ((2023, 2, 28), 12 * 3600));
        assert_eq!(local(ago(ctx, 1, Unit::Years)), ((2023, 3, 31), 12 * 3600));
        assert_eq!(ago(ctx, 3, Unit::Days), now - 3 * 86_400);
        assert_eq!(ago(ctx, 2, Unit::Weeks), now - 14 * 86_400);
        assert_eq!(ctx.day_of(now), days_from_civil(2024, 3, 31));
        assert_eq!(ctx.day_start(ctx.day_of(now)) + tz, days_from_civil(2024, 3, 31) * 86_400);
        // Late in the evening UTC, already the next day at UTC+2.
        let ctx = Ctx::new(days_from_civil(2024, 1, 1) * 86_400 - 3600, tz);
        assert_eq!(ctx.day_of(ctx.now), days_from_civil(2024, 1, 1));
    }

    /// Central European time in 2024: UTC+1, and UTC+2 from 2024-03-31 01:00 UTC (02:00 local
    /// becomes 03:00) until 2024-10-27 01:00 UTC (03:00 local becomes 02:00 again).
    fn cet(t: i64) -> i64 {
        let (spring, autumn) = (days_from_civil(2024, 3, 31) * 86_400 + 3600, days_from_civil(2024, 10, 27) * 86_400 + 3600);
        if (spring..autumn).contains(&t) { 7200 } else { 3600 }
    }

    /// Unix time of a local date and time in `cet`, given the offset it is in.
    fn at(y: i64, m: u32, d: u32, h: i64, offset: i64) -> i64 {
        days_from_civil(y, m, d) * 86_400 + h * 3600 - offset
    }

    #[test]
    fn a_day_on_the_other_side_of_a_daylight_saving_change_starts_at_its_own_midnight() {
        // Now: 2024-06-15 12:00 summer time.
        let ctx = Ctx::with_zone(at(2024, 6, 15, 12, 7200), Zone::Rules(cet));
        assert_eq!(ctx.day_start(days_from_civil(2024, 1, 10)), at(2024, 1, 10, 0, 3600), "winter midnight: 23:00 UTC, not 22:00");
        assert_eq!(ctx.day_start(days_from_civil(2024, 6, 1)), at(2024, 6, 1, 0, 7200));
        // 2024-01-09 23:30 UTC is already the 10th in winter (it would be too with a fixed +2,
        // but 22:30 UTC would not).
        assert_eq!(ctx.day_of(at(2024, 1, 10, 0, 3600) + 1800), days_from_civil(2024, 1, 10));
        assert_eq!(ctx.day_of(at(2024, 1, 10, 0, 3600) - 1800), days_from_civil(2024, 1, 9));
        // The days of the changes themselves: 23 and 25 hours long.
        let len = |y, m, d| ctx.day_start(days_from_civil(y, m, d) + 1) - ctx.day_start(days_from_civil(y, m, d));
        assert_eq!((len(2024, 3, 31), len(2024, 10, 27), len(2024, 6, 1)), (23 * 3600, 25 * 3600, 24 * 3600));
    }

    #[test]
    fn counting_back_across_a_daylight_saving_change_keeps_the_local_time_of_day() {
        // A week back across the spring change: 7 days less an hour.
        let ctx = Ctx::with_zone(at(2024, 4, 3, 12, 7200), Zone::Rules(cet));
        assert_eq!(ago(ctx, 7, Unit::Days), at(2024, 3, 27, 12, 3600));
        assert_eq!(ctx.now - ago(ctx, 1, Unit::Weeks), 7 * 86_400 - 3600);
        // Six months back across the autumn change: summer time then.
        let ctx = Ctx::with_zone(at(2024, 11, 15, 12, 3600), Zone::Rules(cet));
        assert_eq!(ago(ctx, 6, Unit::Months), at(2024, 5, 15, 12, 7200));
        assert_eq!(ago(ctx, 30, Unit::Days), at(2024, 10, 16, 12, 7200));
        // Within one offset, as before: exact.
        let ctx = Ctx::with_zone(at(2024, 7, 20, 12, 7200), Zone::Rules(cet));
        assert_eq!(ago(ctx, 3, Unit::Days), ctx.now - 3 * 86_400);
    }

    #[test]
    fn a_local_time_the_clock_skipped_or_repeated_has_one_instant() {
        let ctx = Ctx::with_zone(at(2024, 6, 15, 12, 7200), Zone::Rules(cet));
        // 02:30 on 2024-03-31 never happened: the first instant after it is 03:00 summer time.
        let gap = days_from_civil(2024, 3, 31) * 86_400 + 2 * 3600 + 1800;
        assert_eq!(ctx.instant(gap), at(2024, 3, 31, 3, 7200));
        // 02:30 on 2024-10-27 happened twice: the first time, in summer time.
        let twice = days_from_civil(2024, 10, 27) * 86_400 + 2 * 3600 + 1800;
        assert_eq!(ctx.instant(twice), twice - 7200);
        // A zone whose clock jumps at midnight (UTC-4 to UTC-3 at 00:00): that day starts at 01:00.
        fn midnight(t: i64) -> i64 {
            if t >= days_from_civil(2024, 9, 8) * 86_400 + 4 * 3600 { -3 * 3600 } else { -4 * 3600 }
        }
        let ctx = Ctx::with_zone(days_from_civil(2024, 12, 1) * 86_400, Zone::Rules(midnight));
        let day = days_from_civil(2024, 9, 8);
        assert_eq!(ctx.day_start(day), day * 86_400 + 4 * 3600);
        assert_eq!(ctx.day_of(ctx.day_start(day)), day);
        assert_eq!(ctx.day_of(ctx.day_start(day) - 1), day - 1);
        // A fixed zone is plain arithmetic.
        let fixed = Ctx::new(0, -18_000);
        assert_eq!(fixed.instant(1000), 19_000);
    }
}
