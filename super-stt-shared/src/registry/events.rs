// SPDX-License-Identifier: GPL-3.0-only
//! Event payloads streamed on `/events` for registry install progress.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RegistryEvent {
    #[serde(rename = "registry.install.progress")]
    Progress {
        install_id: String,
        source: String,
        phase: InstallPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes_done: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes_total: Option<u64>,
    },
    #[serde(rename = "registry.install.completed")]
    Completed {
        install_id: String,
        source: String,
        version: String,
    },
    #[serde(rename = "registry.install.failed")]
    Failed {
        install_id: String,
        source: String,
        phase: InstallPhase,
        error: InstallError,
    },
    #[serde(rename = "registry.refresh.completed")]
    RefreshCompleted {
        generated_at: String,
        backend_count: usize,
    },
    #[serde(rename = "registry.refresh.failed")]
    RefreshFailed { error: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallPhase {
    Resolving,
    Downloading,
    Verifying,
    Extracting,
    Installing,
    Rescanning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallError {
    Incompatible,
    DownloadFailed,
    AssetHashMismatch,
    TarballUnsafe,
    InstallIoError,
}
