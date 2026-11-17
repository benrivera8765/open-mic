//! Small serde helpers for reading JSON written by other implementations leniently.
//!
//! The user-state payload is edited by several programs over the life of a song
//! (the Windows app, the Mac app, later versions). A wrongly typed field there must not
//! throw away the singer's other settings, so its fields are read with these helpers:
//! a value of the wrong type falls back to the field's default instead of failing the
//! whole payload.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Deserialises `T`, or `T::default()` when the JSON value has the wrong shape
/// (including `null`).
pub fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let v = Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

/// Like [`lenient`] but for `bool` fields whose default is `true`.
pub fn lenient_bool_true<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Value::deserialize(d)?.as_bool().unwrap_or(true))
}

/// Like [`lenient`] but for `f64` fields whose default is `1.0` (levels, gains).
pub fn lenient_f64_one<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Value::deserialize(d)?.as_f64().filter(|x| x.is_finite()).unwrap_or(1.0))
}

/// A finite `f64`, else 0.0.
pub fn lenient_f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Value::deserialize(d)?.as_f64().filter(|x| x.is_finite()).unwrap_or(0.0))
}

/// An integer that may have been written as a float by another implementation
/// (`2.0` reads as 2); anything else reads as 0.
pub fn lenient_i64<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|x| x.is_finite() && x.abs() < 9.0e15).map(|x| x.round() as i64))
            .unwrap_or(0),
        _ => 0,
    })
}

/// [`lenient_i64`] narrowed to `i32` (saturating).
pub fn lenient_i32<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    let x = lenient_i64(d)?;
    Ok(x.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

/// A map of finite numbers; non-numeric entries are dropped.
pub fn lenient_levels<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<std::collections::BTreeMap<String, f64>, D::Error> {
    let v = Value::deserialize(d)?;
    let mut out = std::collections::BTreeMap::new();
    if let Value::Object(m) = v {
        for (k, v) in m {
            if let Some(x) = v.as_f64().filter(|x| x.is_finite()) {
                out.insert(k, x);
            }
        }
    }
    Ok(out)
}

/// Distinguishes "key absent" (`None`) from "key present and null" (`Some(None)`).
/// Use with `#[serde(default, deserialize_with = "double_option")]`.
pub fn double_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::Null => Some(None),
        Value::String(s) => Some(Some(s)),
        _ => None, // wrong type: treat as "not overridden"
    })
}
