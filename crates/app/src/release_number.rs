//! A release number given from outside (`MP_RELEASE_VERSION`), checked. Not a module of the app:
//! `build.rs` includes it (a release build takes the number from the variable instead of
//! numbering itself) and so does `cargo xtask release` (which refuses a malformed one before
//! anything builds, and tests this file).

/// The variable a release build takes its number from when set (CI sets it: the number is made
/// once per release, so the builds of one release cannot differ across midnight).
pub const VAR: &str = "MP_RELEASE_VERSION";

/// `YYYY.M.D.count` as `build.rs` writes it (no leading zeros, month 1-12, day 1-31, count
/// 1-65535: the parts of the Win32 version resource are 16 bits), as its four numbers.
pub fn parse(text: &str) -> Result<[u16; 4], String> {
    let bad = || format!("{VAR}={text:?} is not a release number (YYYY.M.D.count, no leading zeros)");
    let parts: Vec<&str> = text.split('.').collect();
    let [y, m, d, n] = parts[..] else { return Err(bad()) };
    let number = |p: &str, digits: std::ops::RangeInclusive<usize>| -> Option<u16> {
        let ok = digits.contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()) && !p.starts_with('0');
        if ok { p.parse().ok() } else { None }
    };
    let (Some(y), Some(m), Some(d), Some(n)) = (number(y, 4..=4), number(m, 1..=2), number(d, 1..=2), number(n, 1..=5)) else {
        return Err(bad());
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(bad());
    }
    Ok([y, m, d, n])
}
