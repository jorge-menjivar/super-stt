// SPDX-License-Identifier: GPL-3.0-only
//! The git host ("forge") that publishes a backend's releases. Declared
//! explicitly on every `registry.toml` entry so the indexer knows which API to
//! speak. There is no default and no inference from the host in `repo`: an
//! unrecognized value is a hard parse error, never a silent fallback. Today
//! only GitHub is implemented; new forges are added as enum variants paired
//! with an adapter in the `super-stt-forge` crate.

use serde::{Deserialize, Serialize};

/// The forge hosting a backend's releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Forge {
    /// GitHub (`api.github.com`, or a GitHub Enterprise base via
    /// `GITHUB_API_BASE`). The only forge with an adapter today.
    Github,
}

#[cfg(test)]
mod tests {
    use super::Forge;

    #[derive(serde::Deserialize)]
    struct Wrap {
        forge: Forge,
    }

    #[test]
    fn parses_snake_case_and_rejects_unknown() {
        let w: Wrap = toml::from_str(r#"forge = "github""#).unwrap();
        assert_eq!(w.forge, Forge::Github);
        // Unknown forge → hard error (no fallback).
        assert!(toml::from_str::<Wrap>(r#"forge = "gitlab""#).is_err());
        // Wire form is snake_case only.
        assert!(toml::from_str::<Wrap>(r#"forge = "GitHub""#).is_err());
    }
}
