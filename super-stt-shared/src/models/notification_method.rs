// SPDX-License-Identifier: GPL-3.0-only

use super::wire_enum::wire_enum_strings;

/// How the daemon surfaces a recording failure to the user.
///
/// The caller always learns about a failure through the response and the
/// `error` event; this controls the additional human-facing notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotificationMethod {
    /// Desktop notification, falling back to typing the notice if it cannot
    /// be delivered.
    #[default]
    Auto,
    /// Desktop notification only; log if it cannot be delivered.
    Dbus,
    /// Type a fixed notice into the focused window.
    Typed,
    /// Log only; never surface.
    Off,
}

wire_enum_strings!(NotificationMethod {
    Auto => "auto",
    Dbus => "dbus",
    Typed => "typed",
    Off => "off",
});

impl NotificationMethod {
    #[must_use]
    pub fn pretty_name(self) -> &'static str {
        match self {
            Self::Auto => "Auto (recommended)",
            Self::Dbus => "Desktop notification",
            Self::Typed => "Type into window",
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
        for method in [
            NotificationMethod::Auto,
            NotificationMethod::Dbus,
            NotificationMethod::Typed,
            NotificationMethod::Off,
        ] {
            let s = method.to_string();
            let parsed: NotificationMethod = s.parse().unwrap();
            assert_eq!(parsed, method);
        }
    }

    #[test]
    fn wire_tokens_are_snake_case() {
        assert_eq!(NotificationMethod::Auto.to_string(), "auto");
        assert_eq!(NotificationMethod::Dbus.to_string(), "dbus");
        assert_eq!(NotificationMethod::Typed.to_string(), "typed");
        assert_eq!(NotificationMethod::Off.to_string(), "off");
    }

    #[test]
    fn from_str_rejects_unknown_and_plausible_aliases() {
        assert!("nonsense".parse::<NotificationMethod>().is_err());
        // No aliases: exactly one token maps to each variant.
        for dropped in [
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
        let method = NotificationMethod::Dbus;
        let json = serde_json::to_string(&method).unwrap();
        assert_eq!(json, "\"dbus\"");
        let parsed: NotificationMethod = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, method);
    }

    #[test]
    fn wire_variants_lists_every_token() {
        assert_eq!(
            NotificationMethod::WIRE_VARIANTS,
            &["auto", "dbus", "typed", "off"]
        );
    }
}
