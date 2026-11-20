//! The release number: what `crates/app/build.rs` wrote to `.app-version`, and what the binary
//! says for it.

/// The release number in `.app-version`'s text: `YYYY.M.D.count`, four numbers. A dev build's
/// `YYYY.M.D-dev`, an empty file or anything else is refused.
pub fn release(text: &str) -> Result<String, String> {
    let v = text.trim();
    if v.is_empty() {
        return Err("empty: no release build numbered it".into());
    }
    if v.ends_with("-dev") {
        return Err(format!("{v}: a development build's number, not a release's"));
    }
    let parts: Vec<&str> = v.split('.').collect();
    let numbers = parts.iter().all(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok());
    if parts.len() != 4 || !numbers || parts[0].len() != 4 {
        return Err(format!("{v}: not a release number (YYYY.M.D.count)"));
    }
    Ok(v.to_owned())
}

// The check `crates/app/build.rs` makes of a release number given in `MP_RELEASE_VERSION`.
#[path = "../../../app/src/release_number.rs"]
mod given;

/// `MP_RELEASE_VERSION`.
pub use given::VAR as GIVEN_VAR;

/// `MP_RELEASE_VERSION`, when set and not empty: the number the release build will take, checked
/// as `build.rs` checks it, so that a malformed one is refused before the tests and the build.
pub fn given(value: Option<&str>) -> Result<Option<String>, String> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => given::parse(v).map(|_| Some(v.to_owned())),
    }
}

/// What `MarqueePlayer --version` prints for release `v`.
pub fn says(v: &str) -> String {
    format!("Marquee Player {v}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_numbers() {
        assert_eq!(release("2026.11.17.4\r\n").as_deref(), Ok("2026.11.17.4"));
        assert_eq!(release(" 2026.1.2.13 ").as_deref(), Ok("2026.1.2.13"));
        for bad in ["", "  \n", "2026.11.17-dev", "2026.11.17", "2026.11.17.4.1", "2026.11.x.4", "2026..17.4", "26.11.17.4", "2026.11.17.70000"] {
            assert!(release(bad).is_err(), "{bad:?} was taken");
        }
        assert_eq!(says("2026.11.17.4"), "Marquee Player 2026.11.17.4");
    }

    #[test]
    fn given_numbers() {
        assert_eq!(given(None), Ok(None));
        assert_eq!(given(Some("")), Ok(None));
        assert_eq!(given(Some("  ")), Ok(None));
        assert_eq!(given(Some("2026.11.20.1")), Ok(Some("2026.11.20.1".to_owned())));
        assert_eq!(given(Some(" 2026.1.5.3\r\n")), Ok(Some("2026.1.5.3".to_owned())));
        assert_eq!(given::parse("2026.12.31.65535"), Ok([2026, 12, 31, 65535]));
        assert_eq!(given::parse("2026.1.1.1"), Ok([2026, 1, 1, 1]));
        for bad in [
            "2026.11.20", "2026.11.20.1.1", "2026.11.20-dev", "v2026.11.20.1", "2026.01.5.3", "2026.1.05.3", "2026.1.5.03",
            "2026.0.5.3", "2026.13.5.3", "2026.1.0.3", "2026.1.32.3", "2026.1.5.0", "2026.1.5.65536", "2026.1.5.123456",
            "026.1.5.3", "20266.1.5.3", "0226.1.5.3", "2026..5.3", "2026.1.5.", "2026.+1.5.3", "2026.1.5.3 ", "x",
        ] {
            assert!(given::parse(bad).is_err(), "{bad:?} was taken");
        }
        assert!(given(Some("2026.13.5.3")).unwrap_err().contains("MP_RELEASE_VERSION"));
    }
}
