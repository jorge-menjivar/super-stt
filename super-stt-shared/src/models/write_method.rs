// SPDX-License-Identifier: GPL-3.0-only

use super::wire_enum::wire_enum_strings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WriteMethod {
    /// Auto-detect: try XDG Portal, then ydotool, then wayland-protocol.
    #[default]
    Auto,
    /// XDG Desktop Portal `RemoteDesktop` keyboard input.
    XdgDesktopPortal,
    /// ydotool virtual input (requires ydotoold running).
    Ydotool,
    /// wayland-protocol keyboard simulation.
    WaylandProtocol,
}

wire_enum_strings!(WriteMethod {
    Auto => "auto",
    XdgDesktopPortal => "xdg_desktop_portal",
    Ydotool => "ydotool",
    WaylandProtocol => "wayland_protocol",
});

impl WriteMethod {
    #[must_use]
    pub fn pretty_name(self) -> &'static str {
        match self {
            Self::Auto => "Auto (recommended)",
            Self::XdgDesktopPortal => "XDG Desktop Portal",
            Self::Ydotool => "ydotool",
            Self::WaylandProtocol => "Wayland Protocol",
        }
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
            WriteMethod::WaylandProtocol,
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
        assert_eq!(WriteMethod::WaylandProtocol.to_string(), "wayland_protocol");
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
}
