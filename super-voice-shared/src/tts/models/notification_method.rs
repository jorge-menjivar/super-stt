// SPDX-License-Identifier: GPL-3.0-only

use super_engine_protocol::wire_enum_strings;

/// How the daemon surfaces a synthesis failure to the user.
///
/// The caller always learns about a failure through the response and the
/// `error` event; this controls the additional human-facing notice.
///
/// Two variants, not four: the STT build could also *type* a notice into the
/// focused window, which is why it distinguished "notify, else type" (`auto`)
/// from "notify only" (`dbus`). With audio as the only output there is one
/// channel left, and a setting whose values behave identically is a trap.
/// `Auto` names the intent — surface it however the daemon best can — so a
/// second channel can be added later without another wire value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotificationMethod {
    /// Surface through the best channel available — today, a desktop
    /// notification; log it if none can be reached.
    #[default]
    Auto,
    /// Log only; never surface.
    Off,
}

wire_enum_strings!(NotificationMethod {
    Auto => "auto",
    Off => "off",
});

impl NotificationMethod {
    #[must_use]
    pub fn pretty_name(self) -> &'static str {
        match self {
            Self::Auto => "Notify me (recommended)",
            Self::Off => "Off",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_auto() {
        assert_eq!(NotificationMethod::default(), NotificationMethod::Auto);
    }

    #[test]
    fn display_roundtrip() {
        for method in [NotificationMethod::Auto, NotificationMethod::Off] {
            let s = method.to_string();
            let parsed: NotificationMethod = s.parse().unwrap();
            assert_eq!(parsed, method);
        }
    }

    #[test]
    fn wire_tokens_are_snake_case() {
        assert_eq!(NotificationMethod::Auto.to_string(), "auto");
        assert_eq!(NotificationMethod::Off.to_string(), "off");
    }

    #[test]
    fn from_str_rejects_unknown_and_plausible_aliases() {
        assert!("nonsense".parse::<NotificationMethod>().is_err());
        // No aliases: exactly one token maps to each variant.
        // `dbus` and `typed` were the STT build's channels. They must not
        // linger as accepted aliases: a client that sets `typed` and gets a
        // success would believe failures are being typed somewhere.
        for dropped in [
            "dbus",
            "typed",
            "notification",
            "notify",
            "d-bus",
            "freedesktop",
            "none",
            "disabled",
            "Auto",
        ] {
            assert!(
                dropped.parse::<NotificationMethod>().is_err(),
                "`{dropped}` must not parse"
            );
        }
    }

    #[test]
    fn serde_roundtrip() {
        let method = NotificationMethod::Off;
        let json = serde_json::to_string(&method).unwrap();
        assert_eq!(json, "\"off\"");
        let parsed: NotificationMethod = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, method);
    }

    #[test]
    fn wire_variants_lists_every_token() {
        assert_eq!(NotificationMethod::WIRE_VARIANTS, &["auto", "off"]);
    }
}
