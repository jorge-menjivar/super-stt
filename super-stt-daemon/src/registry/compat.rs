// SPDX-License-Identifier: GPL-3.0-only
//! `select(host, entry)` — pure: no I/O, no shared state. Asset selection is
//! driven solely by host capability (the most optimal asset the host can run);
//! the runtime device preference is intentionally decoupled and never affects
//! which asset is downloaded.

use semver::Version;
use super_stt_registry_types::manifest::Contract;
use super_stt_shared::registry::SelectedAsset;

use crate::registry::host_detect::Host;
use crate::registry::index_schema::{IndexBackend, IndexSubprocessAsset};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Wasm,
    Subprocess {
        index: usize,
    },
    Incompatible {
        reason: String,
    },
    /// This build does not know the entry's contract generation. Distinct
    /// from [`Incompatible`](Self::Incompatible) because the remedy is
    /// different in kind: a host that lacks the right GPU will never run this
    /// asset, but a Super STT that is merely too old is one update away — so
    /// clients surface this even where they hide hardware mismatches.
    NeedsClientUpdate {
        reason: String,
    },
}

impl Selection {
    /// The human-readable reason this entry cannot be installed, or `None`
    /// when it can.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Wasm | Self::Subprocess { .. } => None,
            Self::Incompatible { reason } | Self::NeedsClientUpdate { reason } => Some(reason),
        }
    }

    /// Whether the block is "your Super STT is too old" rather than "this
    /// machine cannot run it".
    #[must_use]
    pub fn needs_client_update(&self) -> bool {
        matches!(self, Self::NeedsClientUpdate { .. })
    }
}

/// Below this major version an AMD architecture's *stepping* is a whole
/// generation rather than a variant, so the same-family fallback must not
/// apply. `gfx900` (Vega 10), `gfx906` (Vega 20), `gfx908` (MI100) and
/// `gfx90a` (MI200) all decode to major 9, minor 0 and are mutually
/// incompatible; from `gfx10` on, steppings within a minor are ISA-compatible,
/// which is what makes one `gfx1030` build serve the whole `gfx103x` line.
pub const GFX_FAMILY_FLOOR: u32 = 10;

/// Rank of an accel family. A native accel beats a portable one, which beats
/// the CPU. CUDA and `ROCm` never compete: a host reports a compute capability
/// or gfx targets, not both.
const RANK_CPU: u8 = 0;
const RANK_VULKAN: u8 = 1;
const RANK_NATIVE: u8 = 2;

/// Whether an asset's declared gfx target can run on a host's.
///
/// Exact always; same-family only from [`GFX_FAMILY_FLOOR`] up.
fn gfx_runs_on(asset: super_stt_registry_types::arch::GfxSpec, host: gpu_probe::GfxTarget) -> bool {
    gfx_is_exact(asset, host)
        || (asset.major >= GFX_FAMILY_FLOOR
            && asset.major == host.major
            && asset.minor == host.minor)
}

/// Whether an asset's declared gfx target is an exact hit, used to rank an
/// exact match above one that relied on the family fallback.
fn gfx_is_exact(
    asset: super_stt_registry_types::arch::GfxSpec,
    host: gpu_probe::GfxTarget,
) -> bool {
    asset.major == host.major && asset.minor == host.minor && asset.step == host.step
}

/// Longest registry-supplied contract string echoed into a reason. A
/// generation is `v` plus digits; anything longer is not one, and the reason
/// is user-facing text assembled from a document this daemon did not write.
const MAX_ECHOED_CONTRACT: usize = 16;

/// Why an entry's contract generation cannot be driven by this daemon, or
/// `None` when it can.
///
/// The generation is the one thing a manifest declares about what it needs
/// from a daemon, and a daemon knows every generation up to the one it was
/// built with — so "does the string parse as a [`Contract`]" is the whole
/// test. The index also stamps the release that introduced the generation, so
/// the reason can say what to update to even for a generation this build has
/// never heard of.
///
/// Both echoed values come from `index.json`, which is fetched — and
/// `SUPER_STT_REGISTRY_URL` can point somewhere else, which is why
/// `retain_safe_backends` already sanitizes the fields that become paths.
/// These two only become text, so they are bounded rather than rejected: the
/// contract is truncated, and `min_client` is quoted only when it parses as
/// semver *and* actually exceeds this build. A floor at or below the running
/// version would otherwise produce advice the user has already followed.
fn contract_block(entry: &IndexBackend) -> Option<String> {
    if entry.contract.parse::<Contract>().is_ok() {
        return None;
    }
    let daemon = crate::registry::index_schema::CLIENT_VERSION;
    let contract: String = entry.contract.chars().take(MAX_ECHOED_CONTRACT).collect();
    let floor = entry.min_client.as_deref().filter(|min| {
        match (Version::parse(min), Version::parse(daemon)) {
            (Ok(min), Ok(running)) => min > running,
            _ => false,
        }
    });
    Some(match floor {
        Some(min) => {
            format!(
                "needs Super STT {min} or newer (backend contract {contract}); this is {daemon}"
            )
        }
        None => format!("needs a newer Super STT (backend contract {contract}); this is {daemon}"),
    })
}

#[must_use]
pub fn select(host: &Host, entry: &IndexBackend) -> Selection {
    // Before any asset question: a backend this daemon cannot drive has no
    // compatible asset, whatever the host looks like. Listing, install and
    // update all route through here, so one check covers all three.
    if let Some(reason) = contract_block(entry) {
        return Selection::NeedsClientUpdate { reason };
    }
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

    // Capability-driven: the most optimal asset the host can run. Independent
    // of the runtime device preference — a GPU build still runs on CPU when
    // the user selects that device.
    // `reduce` keeping a strict improvement, not `max_by_key`: that returns the
    // *last* maximum. First-wins is the intended tiebreak — an asset's position
    // in the manifest is the author's own preference order, and the CPU
    // fallback already behaved this way as a `.find()`. It does change the CUDA
    // path, which used `max_by_key` and so resolved a tie to the *last*
    // declared asset; nothing pinned that, and one rule across every accel is
    // worth more than preserving an accident on one of them.
    let best = by_target
        .iter()
        .filter_map(|(idx, a)| score(host, a).map(|s| (s, *idx)))
        .reduce(|best, next| if next.0 > best.0 { next } else { best });
    if let Some((_, idx)) = best {
        return Selection::Subprocess { index: idx };
    }
    Selection::Incompatible {
        reason: incompatible_reason(host, &by_target),
    }
}

/// Rank an asset against the host, or `None` when it cannot run at all.
///
/// The tuple orders lexicographically, which is the whole preference policy:
/// accel family first, then the family's own discriminators.
fn score(host: &Host, a: &IndexSubprocessAsset) -> Option<(u8, u32, u8, u8)> {
    let declares = |k: &str| a.accel.iter().any(|x| x == k);

    if declares("cuda")
        && let Some(cuda) = &host.cuda
        && (a.cuda_sm.is_none() || a.cuda_sm == Some(cuda.compute_capability))
        && a.cuda_major.is_some_and(|m| m <= cuda.runtime_major)
    {
        return Some((
            RANK_NATIVE,
            a.cuda_major.unwrap_or(0),
            u8::from(a.cuda_sm.is_some()),
            u8::from(a.cudnn && cuda.cudnn_present),
        ));
    }

    if declares("rocm")
        && let Some(rocm) = &host.rocm
    {
        let targets: Vec<_> = a
            .gfx
            .iter()
            .filter_map(|g| g.parse::<super_stt_registry_types::arch::GfxSpec>().ok())
            .collect();
        let mut best: Option<u8> = None;
        for host_target in &rocm.gfx_targets {
            for asset_target in &targets {
                if gfx_runs_on(*asset_target, *host_target) {
                    let exact = u8::from(gfx_is_exact(*asset_target, *host_target));
                    best = Some(best.map_or(exact, |b| b.max(exact)));
                }
            }
        }
        if let Some(exact) = best {
            return Some((RANK_NATIVE, 0, exact, 0));
        }
    }

    if declares("vulkan")
        && let Some(vulkan) = &host.vulkan
    {
        // A declared floor that does not parse fails closed — the asset stops
        // matching, exactly as a bad gfx string drops out of `targets` above.
        // The natural authoring mistake is `vulkan_api = "1.3.0"`, natural
        // because `VulkanApi` has no patch component, and folding that into
        // "no floor declared" would match every Vulkan host including a 1.0
        // one that cannot run the build. Falling through rather than returning
        // leaves any other accel the asset declares free to match.
        let ok = match a.vulkan_api.as_deref() {
            None => true,
            Some(floor) => match floor.parse::<super_stt_registry_types::arch::VulkanApi>() {
                Ok(f) => (vulkan.api_version.major, vulkan.api_version.minor) >= (f.major, f.minor),
                Err(_) => false,
            },
        };
        if ok {
            return Some((RANK_VULKAN, 0, 0, 0));
        }
    }

    if declares("cpu") {
        return Some((RANK_CPU, 0, 0, 0));
    }
    None
}

/// The `Incompatible` reason: what this host offers, and what the candidate
/// builds asked for.
///
/// Both halves are needed. The host's capability alone names whichever axis
/// happened to match and stays silent about the one that failed — an `sm_86`
/// host offered a `cuda_major = 13` build is told `sm_86`, which is true and
/// useless, since its card *is* `sm_86`. Naming what the candidates require
/// turns "here is my host" into something a backend author can act on.
///
/// Only assets already filtered to this target triple reach here, so the
/// requirement list describes real alternatives rather than every asset
/// published.
fn incompatible_reason(host: &Host, candidates: &[(usize, &IndexSubprocessAsset)]) -> String {
    let mut caps = Vec::new();
    if let Some(c) = &host.cuda {
        // The runtime major is reported alongside the SM because it is the
        // axis that excludes a build the card itself could have run.
        caps.push(format!(
            "sm_{}, CUDA {}",
            c.compute_capability, c.runtime_major
        ));
    }
    if let Some(r) = &host.rocm {
        let targets: Vec<String> = r.gfx_targets.iter().map(ToString::to_string).collect();
        // The userspace version is diagnostic only — it never gated selection,
        // but it is the first thing to check when a ROCm asset was expected.
        match r.version {
            Some(v) => caps.push(format!("{} (ROCm {v})", targets.join(","))),
            None => caps.push(format!("{} (no ROCm userspace found)", targets.join(","))),
        }
    }
    if let Some(v) = &host.vulkan {
        caps.push(format!("vulkan {}", v.api_version));
    }
    if caps.is_empty() {
        caps.push("cpu only".into());
    }
    let mut wants: Vec<String> = Vec::new();
    for (_, a) in candidates {
        let want = asset_requirement(a);
        if !wants.contains(&want) {
            wants.push(want);
        }
    }
    let host_caps = caps.join("; ");
    if wants.is_empty() {
        format!(
            "no compatible asset for host `{}`: {host_caps}",
            host.target_triple
        )
    } else {
        format!(
            "no compatible asset for host `{}`: {host_caps} — candidate builds require: {}",
            host.target_triple,
            wants.join(", ")
        )
    }
}

/// What one candidate asset needs, in the vocabulary its manifest declares it
/// in, so the requirement reads back as the field the author would edit.
fn asset_requirement(a: &IndexSubprocessAsset) -> String {
    let mut parts = Vec::new();
    for accel in &a.accel {
        parts.push(match accel.as_str() {
            "cuda" => {
                let major = a
                    .cuda_major
                    .map_or_else(|| "?".to_string(), |m| m.to_string());
                let sm = a.cuda_sm.map_or_else(|| "*".to_string(), |s| s.to_string());
                format!("cuda {major} sm_{sm}")
            }
            "rocm" => format!("rocm {}", a.gfx.join(",")),
            "vulkan" => match a.vulkan_api.as_deref() {
                Some(v) => format!("vulkan {v}+"),
                None => "vulkan".to_string(),
            },
            other => other.to_string(),
        });
    }
    if parts.is_empty() {
        "an unspecified accel".to_string()
    } else {
        parts.join("+")
    }
}

#[must_use]
pub fn to_selected_asset(entry: &IndexBackend, sel: &Selection) -> Option<SelectedAsset> {
    match sel {
        Selection::Wasm => entry.assets.wasm.as_ref().map(|_| SelectedAsset {
            target: String::new(),
            accel: vec!["wasm".into()],
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
        Selection::Incompatible { .. } | Selection::NeedsClientUpdate { .. } => None,
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
            backend_id: None,
            source: "x".into(),
            version: "1.0.0".into(),
            tag: "v1.0.0".into(),
            name: "T".into(),
            description: None,
            license: "Apache-2.0".into(),
            kind: kind.into(),
            contract: "v1".into(),
            min_client: None,
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
            accel: vec![accel.into()],
            cuda_major: cm,
            cuda_sm: sm,
            cudnn,
            gfx: Vec::new(),
            vulkan_api: None,
            url: Some("x".into()),
            size: Some(1),
            sha256: Some("x".into()),
            parts: Vec::new(),
        }
    }

    fn sp_rocm(target: &str, gfx: &[&str]) -> IndexSubprocessAsset {
        IndexSubprocessAsset {
            target: target.into(),
            accel: vec!["rocm".into()],
            cuda_major: None,
            cuda_sm: None,
            cudnn: false,
            gfx: gfx.iter().map(|g| (*g).to_string()).collect(),
            vulkan_api: None,
            url: Some("x".into()),
            size: Some(1),
            sha256: Some("x".into()),
            parts: Vec::new(),
        }
    }

    fn host_rocm(gfx: &[(u32, u32, u32)]) -> Host {
        Host {
            target_triple: "x86_64-unknown-linux-gnu".into(),
            cuda: None,
            rocm: Some(crate::registry::host_detect::RocmHost {
                gfx_targets: gfx
                    .iter()
                    .map(|(a, b, c)| gpu_probe::GfxTarget::new(*a, *b, *c))
                    .collect(),
                version: None,
            }),
            vulkan: None,
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
            rocm: None,
            vulkan: None,
        }
    }

    fn host_cpu() -> Host {
        Host {
            target_triple: "x86_64-unknown-linux-gnu".into(),
            cuda: None,
            rocm: None,
            vulkan: None,
        }
    }

    #[test]
    fn cpu_host_without_gpu_picks_cpu() {
        // Capability-driven: with no GPU on the host, the CPU asset is selected
        // even though a matching CUDA asset exists.
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                sp(
                    "x86_64-unknown-linux-gnu",
                    "cuda",
                    Some(86),
                    Some(13),
                    false,
                ),
            ],
        );
        let sel = select(&host_cpu(), &e);
        assert_eq!(sel, Selection::Subprocess { index: 0 });
    }

    #[test]
    fn picks_matching_cuda_on_capable_host() {
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
        let sel = select(&host_cuda(86, 12, false), &e);
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
        let sel = select(&host_cuda(86, 12, false), &e);
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
        let sel = select(&host_cuda(86, 12, true), &e);
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
        let sel = select(&host_cuda(86, 13, false), &e);
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
        let sel = select(&host_cuda(86, 12, false), &e);
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
        let sel = select(&host_cuda(120, 13, false), &e);
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
        let sel = select(&host_cuda(90, 13, false), &e);
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
        let sel = select(&host_cuda(86, 12, false), &e);
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
        let sel = select(&host_cuda(86, 12, false), &e);
        assert_eq!(
            sel,
            Selection::Subprocess { index: 0 },
            "a cuda asset with no cuda_major must be ignored, not match every host"
        );
    }

    #[test]
    fn an_exact_gfx_target_matches() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                sp_rocm("x86_64-unknown-linux-gnu", &["gfx1030"]),
            ],
        );
        assert_eq!(
            select(&host_rocm(&[(10, 3, 0)]), &e),
            Selection::Subprocess { index: 1 }
        );
    }

    /// Steppings within a minor are ISA-compatible on RDNA — this is the
    /// `HSA_OVERRIDE_GFX_VERSION=10.3.0` practice that makes one gfx1030 build
    /// serve the whole gfx103x line.
    #[test]
    fn a_same_family_gfx_target_matches_on_rdna() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                sp_rocm("x86_64-unknown-linux-gnu", &["gfx1030"]),
            ],
        );
        assert_eq!(
            select(&host_rocm(&[(10, 3, 1)]), &e),
            Selection::Subprocess { index: 1 },
            "gfx1031 must take the gfx1030 build"
        );
        let e11 = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                sp_rocm("x86_64-unknown-linux-gnu", &["gfx1100"]),
            ],
        );
        assert_eq!(
            select(&host_rocm(&[(11, 0, 1)]), &e11),
            Selection::Subprocess { index: 1 },
            "gfx1101 must take the gfx1100 build"
        );
    }

    /// On CDNA and Vega the *step* is a whole generation: gfx900 (Vega 10),
    /// gfx906 (Vega 20), gfx908 (MI100) and gfx90a (MI200) all decode to
    /// major 9, minor 0 while being mutually incompatible. An unguarded
    /// family rule would hand an MI100 an MI200 build.
    #[test]
    fn the_family_fallback_does_not_apply_to_gfx9() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                sp_rocm("x86_64-unknown-linux-gnu", &["gfx90a"]),
            ],
        );
        assert_eq!(
            select(&host_rocm(&[(9, 0, 8)]), &e),
            Selection::Subprocess { index: 0 },
            "an MI100 must fall back to CPU, never take an MI200 build"
        );
        assert_eq!(
            select(&host_rocm(&[(9, 0, 10)]), &e),
            Selection::Subprocess { index: 1 },
            "an exact gfx90a match is still fine"
        );
    }

    #[test]
    fn an_exact_gfx_match_outranks_a_family_match() {
        let e = entry(
            "subprocess",
            vec![
                sp_rocm("x86_64-unknown-linux-gnu", &["gfx1030"]),
                sp_rocm("x86_64-unknown-linux-gnu", &["gfx1031"]),
            ],
        );
        assert_eq!(
            select(&host_rocm(&[(10, 3, 1)]), &e),
            Selection::Subprocess { index: 1 }
        );
    }

    #[test]
    fn a_rocm_host_with_no_matching_gfx_falls_back_to_cpu() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                sp_rocm("x86_64-unknown-linux-gnu", &["gfx1100"]),
            ],
        );
        assert_eq!(
            select(&host_rocm(&[(10, 3, 0)]), &e),
            Selection::Subprocess { index: 0 }
        );
    }

    /// Declaration order breaks a tie. The CPU fallback used to be a `.find()`,
    /// so the first matching asset won; scoring must not quietly move that to
    /// the last one.
    #[test]
    fn equally_ranked_assets_resolve_to_the_first_declared() {
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
            ],
        );
        assert_eq!(select(&host_cpu(), &e), Selection::Subprocess { index: 0 });
    }

    #[test]
    fn a_dual_runtime_asset_matches_on_either_host() {
        let mut dual = sp_rocm("x86_64-unknown-linux-gnu", &["gfx1030"]);
        dual.accel = vec!["cuda".into(), "rocm".into()];
        dual.cuda_major = Some(12);
        dual.cuda_sm = Some(86);
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                dual,
            ],
        );
        assert_eq!(
            select(&host_cuda(86, 12, false), &e),
            Selection::Subprocess { index: 1 }
        );
        assert_eq!(
            select(&host_rocm(&[(10, 3, 0)]), &e),
            Selection::Subprocess { index: 1 }
        );
    }

    #[test]
    fn a_native_accel_outranks_vulkan_which_outranks_cpu() {
        let mut vk = sp("x86_64-unknown-linux-gnu", "cpu", None, None, false);
        vk.accel = vec!["vulkan".into()];
        let e = entry(
            "subprocess",
            vec![
                sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
                vk.clone(),
                sp_rocm("x86_64-unknown-linux-gnu", &["gfx1030"]),
            ],
        );
        let mut host = host_rocm(&[(10, 3, 0)]);
        host.vulkan = Some(crate::registry::host_detect::VulkanHost {
            api_version: gpu_probe::VulkanVersion::new(1, 3, 0),
        });
        assert_eq!(
            select(&host, &e),
            Selection::Subprocess { index: 2 },
            "rocm must win over vulkan"
        );

        let e_no_rocm = entry(
            "subprocess",
            vec![sp("x86_64-unknown-linux-gnu", "cpu", None, None, false), vk],
        );
        assert_eq!(
            select(&host, &e_no_rocm),
            Selection::Subprocess { index: 1 },
            "vulkan must win over cpu"
        );
    }

    #[test]
    fn a_vulkan_asset_respects_its_api_floor() {
        let mut vk = sp("x86_64-unknown-linux-gnu", "cpu", None, None, false);
        vk.accel = vec!["vulkan".into()];
        vk.vulkan_api = Some("1.3".into());
        let e = entry(
            "subprocess",
            vec![sp("x86_64-unknown-linux-gnu", "cpu", None, None, false), vk],
        );
        let mut host = host_cpu();
        host.vulkan = Some(crate::registry::host_detect::VulkanHost {
            api_version: gpu_probe::VulkanVersion::new(1, 2, 0),
        });
        assert_eq!(
            select(&host, &e),
            Selection::Subprocess { index: 0 },
            "a 1.2 host must not take a 1.3 build"
        );
    }

    /// A declared floor that does not parse must fail closed. `1.3.0` is the
    /// natural authoring mistake — `VulkanApi` carries no patch — and treating
    /// it as "no floor declared" would match every Vulkan host.
    #[test]
    fn a_malformed_vulkan_api_floor_fails_closed() {
        let mut vk = sp("x86_64-unknown-linux-gnu", "cpu", None, None, false);
        vk.accel = vec!["vulkan".into()];
        vk.vulkan_api = Some("1.3.0".into());
        let e = entry(
            "subprocess",
            vec![sp("x86_64-unknown-linux-gnu", "cpu", None, None, false), vk],
        );
        let mut host = host_cpu();
        // A host that would clear a well-formed `1.3`, so only the parse can
        // be what rejects the asset.
        host.vulkan = Some(crate::registry::host_detect::VulkanHost {
            api_version: gpu_probe::VulkanVersion::new(1, 3, 0),
        });
        assert_eq!(
            select(&host, &e),
            Selection::Subprocess { index: 0 },
            "an unparseable floor must not be read as no floor at all"
        );
    }

    /// The reason must name the axis that *failed*. An sm_86 host told only
    /// `sm_86` learns nothing — its card is sm_86; the CUDA runtime major is
    /// what excluded the build — and it never learns what the builds wanted.
    #[test]
    fn the_incompatible_reason_names_the_runtime_and_what_the_builds_need() {
        let e = entry(
            "subprocess",
            vec![sp(
                "x86_64-unknown-linux-gnu",
                "cuda",
                Some(86),
                Some(13),
                false,
            )],
        );
        let Selection::Incompatible { reason } = select(&host_cuda(86, 12, false), &e) else {
            panic!("a cuda_major 13 build must not match a CUDA 12 host");
        };
        assert!(reason.contains("sm_86"), "{reason}");
        assert!(
            reason.contains("CUDA 12"),
            "the host's runtime major is the axis that failed: {reason}"
        );
        assert!(
            reason.contains("cuda 13"),
            "the reason must say what the candidate needed: {reason}"
        );
    }

    /// A ROCm host that matched nothing is told its targets and that the
    /// userspace version never gated the decision, pre-empting the "did you
    /// install ROCm?" red herring.
    #[test]
    fn the_incompatible_reason_names_gfx_targets_and_the_rocm_red_herring() {
        let e = entry(
            "subprocess",
            vec![sp_rocm("x86_64-unknown-linux-gnu", &["gfx1100"])],
        );
        let Selection::Incompatible { reason } = select(&host_rocm(&[(10, 3, 0)]), &e) else {
            panic!("a gfx1100 build must not match a gfx1030 host");
        };
        assert!(reason.contains("gfx1030"), "{reason}");
        assert!(reason.contains("no ROCm userspace found"), "{reason}");
        assert!(reason.contains("rocm gfx1100"), "{reason}");
    }

    #[test]
    fn target_mismatch_is_incompatible() {
        let e = entry(
            "subprocess",
            vec![sp("aarch64-unknown-linux-gnu", "cpu", None, None, false)],
        );
        let sel = select(&host_cuda(86, 12, false), &e);
        assert!(matches!(sel, Selection::Incompatible { .. }));
    }

    /// Every generation this build knows selects normally; the gate is only
    /// for the ones it does not.
    #[test]
    fn every_known_contract_selects() {
        for c in Contract::ALL {
            let mut e = entry("wasm", vec![]);
            e.assets.wasm = Some(IndexAsset {
                url: "https://x".into(),
                size: 1,
                sha256: "abc".into(),
            });
            e.contract = c.to_string();
            assert_eq!(select(&host_cpu(), &e), Selection::Wasm, "{c}");
        }
    }

    /// A generation this build does not know is incompatible before any asset
    /// question — even a perfectly good wasm asset cannot help — and the
    /// reason names the release to update to when the index stamped one.
    #[test]
    fn an_unknown_contract_is_incompatible_and_names_the_floor() {
        let mut e = entry("wasm", vec![]);
        e.assets.wasm = Some(IndexAsset {
            url: "https://x".into(),
            size: 1,
            sha256: "abc".into(),
        });
        e.contract = "v99".into();
        e.min_client = Some("9.9.9".into());
        let sel = select(&host_cpu(), &e);
        assert!(
            sel.needs_client_update(),
            "the remedy is a Super STT update"
        );
        let reason = sel
            .reason()
            .expect("a blocked entry states why")
            .to_string();
        assert!(reason.contains("9.9.9"), "{reason}");
        assert!(reason.contains("v99"), "{reason}");
        assert!(
            reason.contains(crate::registry::index_schema::CLIENT_VERSION),
            "{reason}"
        );

        // An index published before the stamp still refuses, just without the
        // number.
        e.min_client = None;
        let reason = select(&host_cpu(), &e)
            .reason()
            .expect("still blocked without a stamp")
            .to_string();
        assert!(reason.contains("newer Super STT"), "{reason}");
    }
}
