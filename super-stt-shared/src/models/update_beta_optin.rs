// SPDX-License-Identifier: GPL-3.0-only
//! Whether self-update checks consider prerelease (beta) releases.

use super::wire_enum::wire_enum_strings;

/// `Auto` resolves to "include prereleases" iff the running version is
/// itself a prerelease, so beta installs track beta and stable installs
/// track stable without any explicit choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdateBetaOptIn {
    #[default]
    Auto,
    Enabled,
    Disabled,
}

wire_enum_strings!(UpdateBetaOptIn {
    Auto => "auto",
    Enabled => "enabled",
    Disabled => "disabled",
});

#[cfg(test)]
mod tests {
    use super::UpdateBetaOptIn;

    #[test]
    fn wire_round_trip() {
        for s in UpdateBetaOptIn::WIRE_VARIANTS {
            let v: UpdateBetaOptIn = s.parse().unwrap();
            assert_eq!(v.to_string(), *s);
        }
    }

    #[test]
    fn rejects_unknown_and_non_snake_case() {
        assert!("Enabled".parse::<UpdateBetaOptIn>().is_err());
        assert!("on".parse::<UpdateBetaOptIn>().is_err());
        assert!("".parse::<UpdateBetaOptIn>().is_err());
    }

    #[test]
    fn default_is_auto() {
        assert_eq!(UpdateBetaOptIn::default(), UpdateBetaOptIn::Auto);
    }
}
