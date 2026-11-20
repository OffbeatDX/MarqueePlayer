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
}
