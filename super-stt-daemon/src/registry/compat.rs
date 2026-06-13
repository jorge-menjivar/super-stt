// SPDX-License-Identifier: GPL-3.0-only
//! `select(host, entry, prefs)` — pure: no I/O, no shared state.

use super_stt_shared::registry::SelectedAsset;

use crate::registry::host_detect::Host;
use crate::registry::index_schema::{IndexBackend, IndexSubprocessAsset};

#[derive(Debug, Clone, Default)]
pub struct Prefs {
    /// User-asked to prefer GPU for this backend. Mirrors today's per-local-
    /// model "Use GPU" checkbox.
    pub prefer_gpu: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Wasm,
    Subprocess { index: usize },
    Incompatible { reason: String },
}

#[must_use]
pub fn select(host: &Host, entry: &IndexBackend, prefs: &Prefs) -> Selection {
    if entry.kind == "wasm" {
        return if entry.assets.wasm.is_some() {
            Selection::Wasm
        } else {
            Selection::Incompatible {
                reason: "wasm backend missing wasm asset".into(),
            }
        };
    }
    if entry.kind != "subprocess" {
        return Selection::Incompatible {
            reason: format!("unknown kind `{}`", entry.kind),
        };
    }
    // Filter by target triple.
    let by_target: Vec<(usize, &IndexSubprocessAsset)> = entry
        .assets
        .subprocess
        .iter()
        .enumerate()
        .filter(|(_, a)| a.target == host.target_triple)
        .collect();
    if by_target.is_empty() {
        return Selection::Incompatible {
            reason: format!("no asset for target `{}`", host.target_triple),
        };
    }

    if prefs.prefer_gpu
        && let Some(cuda) = &host.cuda
    {
        let cuda_matches: Vec<&(usize, &IndexSubprocessAsset)> = by_target
            .iter()
            .filter(|(_, a)| {
                a.accel == "cuda"
                    && (a.cuda_sm.is_none() || a.cuda_sm == Some(cuda.compute_capability))
                    && a.cuda_major.is_some_and(|m| m <= cuda.runtime_major)
            })
            .collect();
        // Preference: highest cuda_major; then exact-SM over wildcard; then cudnn.
        let best = cuda_matches.iter().max_by_key(|(_, a)| {
            (
                a.cuda_major.unwrap_or(0),
                u8::from(a.cuda_sm.is_some()),
                u8::from(a.cudnn && cuda.cudnn_present),
            )
        });
        if let Some(&&(idx, _)) = best {
            return Selection::Subprocess { index: idx };
        }
        // Fall through to CPU.
    }
    // CPU fallback.
    if let Some((idx, _)) = by_target.iter().find(|(_, a)| a.accel == "cpu") {
        return Selection::Subprocess { index: *idx };
    }
    Selection::Incompatible {
        reason: format!(
            "no compatible asset for host `{}`, sm_{}",
            host.target_triple,
            host.cuda
                .as_ref()
                .map_or("?".into(), |c| c.compute_capability.to_string())
        ),
    }
}

#[must_use]
pub fn to_selected_asset(entry: &IndexBackend, sel: &Selection) -> Option<SelectedAsset> {
    match sel {
        Selection::Wasm => entry.assets.wasm.as_ref().map(|_| SelectedAsset {
            target: String::new(),
            accel: "wasm".into(),
            cuda_major: None,
            cuda_sm: None,
            cudnn: false,
        }),
        Selection::Subprocess { index } => {
            entry.assets.subprocess.get(*index).map(|a| SelectedAsset {
                target: a.target.clone(),
                accel: a.accel.clone(),
                cuda_major: a.cuda_major,
                cuda_sm: a.cuda_sm,
                cudnn: a.cudnn,
            })
        }
        Selection::Incompatible { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::host_detect::{CudaHost, Host};
    use crate::registry::index_schema::*;

    fn entry(kind: &str, subprocess: Vec<IndexSubprocessAsset>) -> IndexBackend {
        IndexBackend {
            id: "t".into(),
            source: "x".into(),
            version: "1.0.0".into(),
            tag: "v1.0.0".into(),
            name: "T".into(),
            description: None,
            license: "Apache-2.0".into(),
            kind: kind.into(),
            contract: "v1".into(),
            entrypoint: "t".into(),
            allowed_hosts: vec![],
            online: false,
            supports_gpu: true,
            supports_cpu: true,
            models: vec![],
            secrets: vec![],
            options: vec![],
            assets: IndexAssets {
                wasm: None,
                subprocess,
            },
            index_stale: None,
            manifest: None,
        }
    }

    fn sp(
        target: &str,
        accel: &str,
        sm: Option<u32>,
        cm: Option<u32>,
        cudnn: bool,
    ) -> IndexSubprocessAsset {
        IndexSubprocessAsset {
            target: target.into(),
            accel: accel.into(),
            cuda_major: cm,
            cuda_sm: sm,
            cudnn,
            url: "x".into(),
            size: 1,
            sha256: "x".into(),
        }
    }

    fn host_cuda(sm: u32, cm: u32, cudnn: bool) -> Host {
        Host {
            target_triple: "x86_64-unknown-linux-gnu".into(),
            cuda: Some(CudaHost {
                compute_capability: sm,
                runtime_major: cm,
                cudnn_present: cudnn,
            }),
        }
    }

    #[test]
    fn picks_matching_cuda_when_gpu_preferred() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                sp(
                    "x86_64-unknown-linux-gnu",
                    "cuda",
                    Some(86),
                    Some(12),
                    false,
                ),
                sp(
                    "x86_64-unknown-linux-gnu",
                    "cuda",
                    Some(90),
                    Some(12),
                    false,
                ),
            ],
        );
        let sel = select(&host_cuda(86, 12, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 1 });
    }

    #[test]
    fn falls_back_to_cpu_when_no_sm_match() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                sp(
                    "x86_64-unknown-linux-gnu",
                    "cuda",
                    Some(90),
                    Some(12),
                    false,
                ),
            ],
        );
        let sel = select(&host_cuda(86, 12, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 0 });
    }

    #[test]
    fn prefers_cudnn_when_host_has_it() {
        let e = entry(
            "subprocess",
            vec![
                sp(
                    "x86_64-unknown-linux-gnu",
                    "cuda",
                    Some(86),
                    Some(12),
                    false,
                ),
                sp("x86_64-unknown-linux-gnu", "cuda", Some(86), Some(12), true),
            ],
        );
        let sel = select(&host_cuda(86, 12, true), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 1 });
    }

    #[test]
    fn picks_highest_cuda_major_within_runtime() {
        let e = entry(
            "subprocess",
            vec![
                sp(
                    "x86_64-unknown-linux-gnu",
                    "cuda",
                    Some(86),
                    Some(12),
                    false,
                ),
                sp(
                    "x86_64-unknown-linux-gnu",
                    "cuda",
                    Some(86),
                    Some(13),
                    false,
                ),
            ],
        );
        // Host has CUDA 13 runtime
        let sel = select(&host_cuda(86, 13, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 1 });
    }

    #[test]
    fn cuda_runtime_caps_choice() {
        let e = entry(
            "subprocess",
            vec![
                sp(
                    "x86_64-unknown-linux-gnu",
                    "cuda",
                    Some(86),
                    Some(12),
                    false,
                ),
                sp(
                    "x86_64-unknown-linux-gnu",
                    "cuda",
                    Some(86),
                    Some(13),
                    false,
                ),
            ],
        );
        // Host has CUDA 12 runtime — must not pick the cuda_major=13 build.
        let sel = select(&host_cuda(86, 12, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 0 });
    }

    #[test]
    fn wildcard_cuda_sm_matches_any_compute_capability() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                // No cuda_sm -> wildcard.
                sp("x86_64-unknown-linux-gnu", "cuda", None, Some(13), false),
            ],
        );
        // Host is sm_120 with a CUDA 13 runtime; the wildcard must match.
        let sel = select(&host_cuda(120, 13, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 1 });
    }

    #[test]
    fn exact_cuda_sm_is_preferred_over_wildcard() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cuda", None, Some(13), false),
                sp(
                    "x86_64-unknown-linux-gnu",
                    "cuda",
                    Some(90),
                    Some(13),
                    false,
                ),
            ],
        );
        let sel = select(&host_cuda(90, 13, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(
            sel,
            Selection::Subprocess { index: 1 },
            "an exact-SM asset must win over a wildcard"
        );
    }

    #[test]
    fn wildcard_cuda_still_respects_runtime_major() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                // Wildcard SM but cuda_major=13 — must NOT match a CUDA 12 host.
                sp("x86_64-unknown-linux-gnu", "cuda", None, Some(13), false),
            ],
        );
        let sel = select(&host_cuda(86, 12, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(
            sel,
            Selection::Subprocess { index: 0 },
            "cuda_major>runtime_major must fall back to CPU"
        );
    }

    #[test]
    fn cuda_asset_without_cuda_major_falls_back_to_cpu() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                // Malformed: cuda accel with neither cuda_sm nor cuda_major.
                // Must not match any host — the cuda_major guard excludes it.
                sp("x86_64-unknown-linux-gnu", "cuda", None, None, false),
            ],
        );
        let sel = select(&host_cuda(86, 12, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(
            sel,
            Selection::Subprocess { index: 0 },
            "a cuda asset with no cuda_major must be ignored, not match every host"
        );
    }

    #[test]
    fn target_mismatch_is_incompatible() {
        let e = entry(
            "subprocess",
            vec![sp("aarch64-unknown-linux-gnu", "cpu", None, None, false)],
        );
        let sel = select(&host_cuda(86, 12, false), &e, &Prefs { prefer_gpu: false });
        assert!(matches!(sel, Selection::Incompatible { .. }));
    }
}
