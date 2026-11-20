//! Tolerant field readers for the models: servers differ (Navidrome, Gonic, Airsonic, Ampache,
//! LMS...) and so do their versions, so a field of an unexpected type reads as its default
//! instead of failing the whole answer. Each is used as `deserialize_with` on a field of a
//! struct marked `#[serde(default)]`, so a missing field is its default too.
//!
//! - text: a string as is; a number or bool as text (ids are numbers on some servers); null
//!   or anything else as empty (or None).
//! - numbers: a number, or a string holding one; a float where an integer is wanted is
//!   truncated; out of range, null or anything else is None (or 0).
//! - flags: a bool, "true"/"false", or a number (non-zero is true).
//! - lists: an array, of which items that do not read are skipped (one odd song does not lose
//!   the page); a lone object as a list of one (older servers' JSON did that); else empty.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

pub fn string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(opt_string(d)?.unwrap_or_default())
}

/// Some text, or None (an empty string is None: a `starred` of "" is not starred).
pub fn opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) if !s.is_empty() => Some(s),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

/// A number type a field can hold.
pub trait Num: Sized + Default {
    fn read(v: &Value) -> Option<Self>;
}

macro_rules! int {
    ($($t:ty),*) => {$(
        impl Num for $t {
            fn read(v: &Value) -> Option<Self> {
                match v {
                    Value::Number(n) => n
                        .as_i64()
                        .and_then(|i| <$t>::try_from(i).ok())
                        .or_else(|| n.as_u64().and_then(|u| <$t>::try_from(u).ok()))
                        .or_else(|| n.as_f64().filter(|f| f.is_finite()).and_then(|f| <$t>::try_from(f.trunc() as i64).ok())),
                    Value::String(s) => s.trim().parse().ok(),
                    _ => None,
                }
            }
        }
    )*};
}
int!(u8, u32, u64, i32, i64);

impl Num for f64 {
    fn read(v: &Value) -> Option<Self> {
        match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
        .filter(|f: &f64| f.is_finite())
    }
}

pub fn num<'de, D: Deserializer<'de>, T: Num>(d: D) -> Result<T, D::Error> {
    Ok(T::read(&Value::deserialize(d)?).unwrap_or_default())
}

pub fn opt_num<'de, D: Deserializer<'de>, T: Num>(d: D) -> Result<Option<T>, D::Error> {
    Ok(T::read(&Value::deserialize(d)?))
}

pub fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Bool(b) => b,
        Value::String(s) => s.eq_ignore_ascii_case("true") || s == "1",
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => false,
    })
}

pub fn list<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(items) => {
            let total = items.len();
            let out: Vec<T> = items.into_iter().filter_map(|v| T::deserialize(v).ok()).collect();
            if out.len() < total {
                // The item's text is not logged: it may hold anything.
                log::debug!("subsonic: skipped {} of {total} items that did not read", total - out.len());
            }
            out
        }
        v @ (Value::Object(_) | Value::String(_)) => T::deserialize(v).ok().into_iter().collect(),
        _ => Vec::new(),
    })
}

/// An object, or None for anything else (null, a string where an object belongs).
pub fn opt_object<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Option<T>, D::Error> {
    Ok(match Value::deserialize(d)? {
        v @ Value::Object(_) => T::deserialize(v).ok(),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Deserialize, Default, Debug, PartialEq)]
    #[serde(default)]
    struct T {
        #[serde(deserialize_with = "super::string")]
        s: String,
        #[serde(deserialize_with = "super::opt_string")]
        o: Option<String>,
        #[serde(deserialize_with = "super::opt_num")]
        n: Option<u32>,
        #[serde(deserialize_with = "super::num")]
        c: u32,
        #[serde(deserialize_with = "super::opt_num")]
        f: Option<f64>,
        #[serde(deserialize_with = "super::flag")]
        b: bool,
        #[serde(deserialize_with = "super::list")]
        l: Vec<u8>,
    }

    fn t(json: &str) -> T {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn odd_types_read_as_defaults_or_converted() {
        assert_eq!(t("{}"), T::default());
        let v = t(r#"{"s":12,"o":"","n":"7","c":3.9,"f":"-6.5","b":"true","l":[1,"x",300,2]}"#);
        assert_eq!(v, T { s: "12".into(), o: None, n: Some(7), c: 3, f: Some(-6.5), b: true, l: vec![1, 2] });
        let v = t(r#"{"s":null,"o":5,"n":-1,"c":null,"f":null,"b":1,"l":4}"#);
        assert_eq!(v, T { s: String::new(), o: Some("5".into()), n: None, c: 0, f: None, b: true, l: vec![] });
        assert_eq!(t(r#"{"l":{"x":1}}"#).l, Vec::<u8>::new());
        assert_eq!(t(r#"{"unknown":[1,2,3],"b":false}"#), T::default());
    }
}
