// SPDX-License-Identifier: GPL-3.0-only
//! Architecture and API-version values a subprocess asset declares.
//!
//! These are manifest vocabulary, not probe results. They are defined here
//! rather than reused from `gpu-probe` because this crate is a dependency of
//! the app, the forge, and the indexer, none of which should link a GPU
//! detection library. The daemon compares these against probed values in
//! `registry::compat`, which is the one place the two vocabularies meet.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// The AMD architecture a `ROCm`/HIP code object is built for, e.g. `gfx1030`.
///
/// Spelled as `--offload-arch` spells it: a decimal major, then a single hex
/// digit each for minor and stepping. `gfx90a` is therefore
/// `{ major: 9, minor: 0, step: 10 }`.
///
/// Ordered `major` first, so targets within a vendor can be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(with = "String"))]
pub struct GfxSpec {
    /// Major version — the `10` in `gfx1030`.
    pub major: u32,
    /// Minor version — the `3` in `gfx1030`.
    pub minor: u32,
    /// Stepping — the `0` in `gfx1030`, and the `a` in `gfx90a`.
    pub step: u32,
}

impl GfxSpec {
    /// Create a target from its major, minor, and stepping parts.
    #[must_use]
    pub const fn new(major: u32, minor: u32, step: u32) -> Self {
        Self { major, minor, step }
    }
}

impl fmt::Display for GfxSpec {
    /// Renders as `gfx1030`, matching `--offload-arch`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gfx{}{:x}{:x}", self.major, self.minor, self.step)
    }
}

impl FromStr for GfxSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let digits = s
            .strip_prefix("gfx")
            .ok_or_else(|| format!("not a gfx target: {s}"))?;
        // The last two characters are the hex minor and stepping; everything
        // before them is the decimal major. Splitting from the right is what
        // makes `gfx1030` and `gfx90a` parse under one rule.
        if digits.len() < 3 {
            return Err(format!("gfx target too short: {s}"));
        }
        let (major, tail) = digits.split_at(digits.len() - 2);
        let mut tail = tail.chars();
        let hex = |c: Option<char>| -> Result<u32, String> {
            c.and_then(|c| c.to_digit(16))
                .ok_or_else(|| format!("bad gfx target: {s}"))
        };
        let minor = hex(tail.next())?;
        let step = hex(tail.next())?;
        let major = major
            .parse()
            .map_err(|_| format!("bad gfx major in: {s}"))?;
        Ok(Self { major, minor, step })
    }
}

/// A minimum Vulkan API version an asset requires, e.g. `1.3`.
///
/// Ordered `major` first. Patch is deliberately absent: drivers advertise
/// feature levels at `major.minor`, and a patch floor would express a
/// requirement no build actually has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(with = "String"))]
pub struct VulkanApi {
    /// Major version — the `1` in `1.3`.
    pub major: u32,
    /// Minor version — the `3` in `1.3`.
    pub minor: u32,
}

impl VulkanApi {
    /// Create an API version from its major and minor parts.
    #[must_use]
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for VulkanApi {
    /// Renders as `1.3`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for VulkanApi {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (major, minor) = s
            .split_once('.')
            .ok_or_else(|| format!("not a major.minor version: {s}"))?;
        Ok(Self {
            major: major
                .trim()
                .parse()
                .map_err(|_| format!("bad vulkan major in: {s}"))?,
            minor: minor
                .trim()
                .parse()
                .map_err(|_| format!("bad vulkan minor in: {s}"))?,
        })
    }
}

/// Both types are strings on the wire and in TOML, so serialization routes
/// through `Display`/`FromStr` rather than deriving a struct shape.
macro_rules! string_serde {
    ($ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let text = String::deserialize(d)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

string_serde!(GfxSpec);
string_serde!(VulkanApi);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_offload_arch_spelling() {
        assert_eq!("gfx1030".parse(), Ok(GfxSpec::new(10, 3, 0)));
        assert_eq!("gfx1013".parse(), Ok(GfxSpec::new(10, 1, 3)));
        // MI200: the trailing `a` is a hex stepping of 10, not a letter suffix.
        assert_eq!("gfx90a".parse(), Ok(GfxSpec::new(9, 0, 10)));
        assert_eq!("gfx1100".parse(), Ok(GfxSpec::new(11, 0, 0)));
    }

    #[test]
    fn round_trips_through_display() {
        for text in ["gfx900", "gfx90a", "gfx1013", "gfx1030", "gfx1100"] {
            let spec: GfxSpec = text.parse().expect("parses");
            assert_eq!(spec.to_string(), text, "round trip for {text}");
        }
    }

    #[test]
    fn rejects_malformed_targets() {
        // The prefix is mandatory, and at least three digits must follow it:
        // one or more for major, one each for minor and stepping.
        assert!("1030".parse::<GfxSpec>().is_err());
        assert!("gfx".parse::<GfxSpec>().is_err());
        assert!("gfx10".parse::<GfxSpec>().is_err());
        assert!("gfxzzzz".parse::<GfxSpec>().is_err());
        assert!("sm_86".parse::<GfxSpec>().is_err());
    }

    #[test]
    fn gfx_orders_major_first() {
        assert!(GfxSpec::new(10, 3, 0) > GfxSpec::new(10, 1, 3));
        assert!(GfxSpec::new(11, 0, 0) > GfxSpec::new(10, 3, 0));
    }

    #[test]
    fn parses_a_vulkan_api_floor() {
        assert_eq!("1.3".parse(), Ok(VulkanApi::new(1, 3)));
        assert_eq!("1.0".parse(), Ok(VulkanApi::new(1, 0)));
        assert_eq!(VulkanApi::new(1, 3).to_string(), "1.3");
        assert!("1".parse::<VulkanApi>().is_err());
        assert!("one.three".parse::<VulkanApi>().is_err());
    }

    #[test]
    fn both_types_survive_a_serde_round_trip() {
        let gfx: GfxSpec = serde_json::from_str("\"gfx1030\"").expect("deserializes");
        assert_eq!(gfx, GfxSpec::new(10, 3, 0));
        assert_eq!(
            serde_json::to_string(&gfx).expect("serializes"),
            "\"gfx1030\""
        );

        let api: VulkanApi = serde_json::from_str("\"1.3\"").expect("deserializes");
        assert_eq!(api, VulkanApi::new(1, 3));
        assert_eq!(serde_json::to_string(&api).expect("serializes"), "\"1.3\"");
    }
}
