// SPDX-License-Identifier: GPL-3.0-only
use log::warn;
use serde::{Deserialize, Deserializer};

/// Deserialize a value via its serde impl, falling back to `Default` if the
/// stored representation is no longer recognized (e.g. a config field written
/// by an older build using a stale format). Logs a warning so the migration is
/// observable. Used on individual enum/struct fields so a single unrecognized
/// value degrades that field to its default instead of failing the whole
/// config parse.
///
/// # Errors
///
/// Returns an error only if the underlying value cannot be captured at all
/// (malformed input); an unrecognized-but-well-formed value yields the default.
pub fn deserialize_or_default<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + serde::de::DeserializeOwned,
    D: Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    match serde_json::from_value::<T>(raw.clone()) {
        Ok(value) => Ok(value),
        Err(e) => {
            warn!("config field {raw} unrecognized ({e}); using default");
            Ok(T::default())
        }
    }
}
