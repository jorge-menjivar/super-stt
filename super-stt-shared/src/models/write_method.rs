// SPDX-License-Identifier: GPL-3.0-only

use super::wire_enum::wire_enum_strings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WriteMethod {
    /// Auto-detect: use the first method the session supports. On Linux that
    /// is built-in, then XDG Portal, then ydotool; on macOS built-in is the
    /// only one, so `Auto` always resolves to it.
    #[default]
    Auto,
    /// XDG Desktop Portal `RemoteDesktop` keyboard input. Linux only.
    XdgDesktopPortal,
    /// ydotool virtual input (requires ydotoold running). Linux only.
    Ydotool,
    /// Keyboard simulation the daemon performs itself, with no helper process
    /// and nothing for the user to install.
    ///
    /// Named for what it is to the user rather than for how it is done,
    /// because how it is done differs by platform and the choice does not: on
    /// Linux it drives `zwp_virtual_keyboard_manager_v1` on the Wayland
    /// session, on macOS it posts CoreGraphics key events. A user picking
    /// between this and ydotool is choosing "Super STT types it" over "a
    /// daemon I installed types it", which is the same choice on both.
    BuiltIn,
}

wire_enum_strings!(WriteMethod {
    Auto => "auto",
    XdgDesktopPortal => "xdg_desktop_portal",
    Ydotool => "ydotool",
    BuiltIn => "built_in",
});

impl WriteMethod {
    #[must_use]
    pub fn pretty_name(self) -> &'static str {
        match self {
            Self::Auto => "Auto (recommended)",
            Self::XdgDesktopPortal => "XDG Desktop Portal",
            Self::Ydotool => "ydotool",
            Self::BuiltIn => "Built-in",
        }
    }

    /// Whether this daemon build can drive the method at all.
    ///
    /// A method this platform has no backend for is not merely unavailable at
    /// runtime — it can never become available, so a client offering it as a
    /// choice is offering a setting that cannot work. Clients filter their
    /// picker through this; the daemon rejects a request for a method it
    /// fails here.
    #[must_use]
    pub fn is_supported_on_this_platform(self) -> bool {
        match self {
            Self::Auto | Self::BuiltIn => true,
            // Both need a Linux session bus or a Linux uinput daemon.
            Self::XdgDesktopPortal | Self::Ydotool => cfg!(target_os = "linux"),
        }
    }

    /// Every method this build can actually drive, in preference order.
    #[must_use]
    pub fn supported_on_this_platform() -> Vec<Self> {
        [
            Self::Auto,
            Self::BuiltIn,
            Self::XdgDesktopPortal,
            Self::Ydotool,
        ]
        .into_iter()
        .filter(|m| m.is_supported_on_this_platform())
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_auto() {
        assert_eq!(WriteMethod::default(), WriteMethod::Auto);
    }

    #[test]
    fn display_roundtrip() {
        for method in [
            WriteMethod::Auto,
            WriteMethod::XdgDesktopPortal,
            WriteMethod::Ydotool,
            WriteMethod::BuiltIn,
        ] {
            let s = method.to_string();
            let parsed: WriteMethod = s.parse().unwrap();
            assert_eq!(parsed, method);
        }
    }

    #[test]
    fn wire_tokens_are_snake_case() {
        assert_eq!(WriteMethod::Auto.to_string(), "auto");
        assert_eq!(
            WriteMethod::XdgDesktopPortal.to_string(),
            "xdg_desktop_portal"
        );
        assert_eq!(WriteMethod::BuiltIn.to_string(), "built_in");
    }

    #[test]
    fn from_str_rejects_unknown_and_dropped_aliases() {
        assert!("nonsense".parse::<WriteMethod>().is_err());
        // Former aliases + kebab forms are gone (no legacy aliases).
        for dropped in [
            "xdg",
            "portal",
            "wayland",
            "xdg-desktop-portal",
            "wayland-protocol",
            // `BuiltIn`'s former name. Only the daemon's config loader still
            // reads it, so a choice stored before the rename survives.
            "wayland_protocol",
        ] {
            assert!(
                dropped.parse::<WriteMethod>().is_err(),
                "`{dropped}` must no longer parse"
            );
        }
    }

    #[test]
    fn serde_roundtrip() {
        let method = WriteMethod::Ydotool;
        let json = serde_json::to_string(&method).unwrap();
        let parsed: WriteMethod = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, method);
    }

    /// `Auto` and `BuiltIn` work everywhere the daemon builds; the other two
    /// are Linux session technologies. A client that offered ydotool on macOS
    /// would be offering a setting no backend can ever satisfy.
    #[test]
    fn platform_support_matches_the_backends_that_exist() {
        assert!(WriteMethod::Auto.is_supported_on_this_platform());
        assert!(WriteMethod::BuiltIn.is_supported_on_this_platform());

        let supported = WriteMethod::supported_on_this_platform();
        assert!(supported.contains(&WriteMethod::Auto));
        assert!(supported.contains(&WriteMethod::BuiltIn));

        #[cfg(target_os = "linux")]
        {
            assert_eq!(supported.len(), 4);
            assert!(supported.contains(&WriteMethod::XdgDesktopPortal));
            assert!(supported.contains(&WriteMethod::Ydotool));
        }
        #[cfg(target_os = "macos")]
        {
            assert_eq!(supported, vec![WriteMethod::Auto, WriteMethod::BuiltIn]);
            assert!(!WriteMethod::XdgDesktopPortal.is_supported_on_this_platform());
            assert!(!WriteMethod::Ydotool.is_supported_on_this_platform());
        }
    }
}
