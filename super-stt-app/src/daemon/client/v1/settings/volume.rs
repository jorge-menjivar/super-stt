// SPDX-License-Identifier: GPL-3.0-only
//! `/volume` — audio-cue master volume (0–100).

settings_getter!(
    get_volume -> u8, "/volume", "get_volume",
    |resp| parse_volume(resp.message.as_deref())
);
settings_setter!(set_volume, volume: u8, "/volume", "volume", "set_volume");

/// Parse the daemon's `message` field ("Volume is 75") into a 0–100 level,
/// falling back to 100 when the field is absent or does not end in a valid
/// `u8` (e.g. empty, non-numeric, or out of range).
fn parse_volume(message: Option<&str>) -> u8 {
    message
        .unwrap_or_default()
        .rsplit(' ')
        .next()
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::parse_volume;

    #[test]
    fn parses_trailing_integer() {
        assert_eq!(parse_volume(Some("Volume is 75")), 75);
        assert_eq!(parse_volume(Some("0")), 0);
        assert_eq!(parse_volume(Some("100")), 100);
    }

    #[test]
    fn falls_back_to_100_when_absent_or_unparseable() {
        assert_eq!(parse_volume(None), 100);
        assert_eq!(parse_volume(Some("")), 100);
        assert_eq!(parse_volume(Some("Volume is loud")), 100);
        // 999 overflows u8 → parse fails → fallback.
        assert_eq!(parse_volume(Some("999")), 100);
    }
}
