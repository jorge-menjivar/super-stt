// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
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

impl std::fmt::Display for WriteMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::XdgDesktopPortal => write!(f, "xdg-desktop-portal"),
            Self::Ydotool => write!(f, "ydotool"),
            Self::WaylandProtocol => write!(f, "wayland-protocol"),
        }
    }
}

impl std::str::FromStr for WriteMethod {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "xdg-desktop-portal" | "xdg" | "portal" => Ok(Self::XdgDesktopPortal),
            "ydotool" => Ok(Self::Ydotool),
            "wayland-protocol" | "wayland" => Ok(Self::WaylandProtocol),
            other => Err(format!("unknown write method: {other}")),
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
    fn from_str_aliases() {
        assert_eq!(
            "xdg".parse::<WriteMethod>().unwrap(),
            WriteMethod::XdgDesktopPortal
        );
        assert_eq!(
            "portal".parse::<WriteMethod>().unwrap(),
            WriteMethod::XdgDesktopPortal
        );
    }

    #[test]
    fn from_str_invalid() {
        assert!("nonsense".parse::<WriteMethod>().is_err());
    }

    #[test]
    fn serde_roundtrip() {
        let method = WriteMethod::Ydotool;
        let json = serde_json::to_string(&method).unwrap();
        let parsed: WriteMethod = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, method);
    }
}
