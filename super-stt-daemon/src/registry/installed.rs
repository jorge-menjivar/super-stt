// SPDX-License-Identifier: GPL-3.0-only
//! The record of which asset variant is installed in a backend directory.
//!
//! `backend.toml` lists every variant a release offers, so it cannot answer
//! "which one is on this machine". The install pipeline knows — it chose one —
//! and writes the answer here so the question survives a restart.
//!
//! This is what lets the offered device list be honest: a CUDA-only backend on
//! an AMD host installs its CPU variant, and a device list derived from this
//! record offers the CPU alone, where one derived from the manifest would
//! offer a GPU that cannot be used.

use serde::{Deserialize, Serialize};
use std::path::Path;
use super_stt_shared::registry::SelectedAsset;

/// Filename inside the backend directory. Sits beside `backend.toml`.
pub const RECORD_FILE: &str = "installed.json";

/// What the install pipeline recorded about this backend directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// The asset variant that was downloaded and unpacked here.
    pub selected: SelectedAsset,
}

/// Write the record into a staged backend directory.
///
/// Called before the staging directory is swapped into place, so the record
/// lands atomically with the payload it describes and no reader can observe a
/// directory whose record disagrees with its binary.
///
/// # Errors
/// Returns an `io::Error` if the record cannot be serialized or the file
/// cannot be written.
pub fn write(dir: &Path, asset: &SelectedAsset) -> std::io::Result<()> {
    let record = Record {
        selected: asset.clone(),
    };
    let text = serde_json::to_vec_pretty(&record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(dir.join(RECORD_FILE), text)
}

/// Read the record, or `None` when there is none to read.
///
/// `None` is a normal answer, not an error: a backend imported from a local
/// directory never had a selection, and a backend installed before this record
/// existed has no file. Both fall back to the manifest's declared devices. A
/// malformed file reads as absent for the same reason — a corrupt record must
/// not take a working backend out of the catalog.
#[must_use]
pub fn read(dir: &Path) -> Option<Record> {
    let text = std::fs::read_to_string(dir.join(RECORD_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(accel: &[&str]) -> SelectedAsset {
        SelectedAsset {
            target: "x86_64-unknown-linux-gnu".into(),
            accel: accel.iter().map(|a| (*a).to_string()).collect(),
            cuda_major: None,
            cuda_sm: None,
            cudnn: false,
        }
    }

    #[test]
    fn a_record_round_trips_through_the_backend_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(read(dir.path()).is_none(), "an empty dir has no record");
        write(dir.path(), &asset(&["cuda"])).expect("writes");
        let record = read(dir.path()).expect("reads back");
        assert_eq!(record.selected.accel, vec!["cuda".to_string()]);
    }

    #[test]
    fn a_malformed_record_reads_as_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(RECORD_FILE), b"not json").expect("writes");
        assert!(
            read(dir.path()).is_none(),
            "a corrupt record must degrade to the manifest fallback, not fail the scan"
        );
    }

    /// A scalar `accel` is what a record written by an older daemon carries,
    /// and what a registry payload may still serve.
    #[test]
    fn a_scalar_accel_in_a_record_reads_as_a_one_element_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(RECORD_FILE),
            br#"{"selected":{"target":"x86_64-unknown-linux-gnu","accel":"cuda"}}"#,
        )
        .expect("writes");
        let record = read(dir.path()).expect("reads back");
        assert_eq!(record.selected.accel, vec!["cuda".to_string()]);
    }
}
