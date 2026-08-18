// SPDX-License-Identifier: GPL-3.0-only
//! Detect the host's target triple and its accelerator capability — CUDA
//! compute capability, runtime CUDA major version and cuDNN presence; the AMD
//! architecture targets and `ROCm` userspace release; the Vulkan runtime's API
//! version. Used by `compat::select` to pick a compatible asset, and surfaced
//! in the install failure error path.

#[derive(Debug, Clone)]
pub struct Host {
    pub target_triple: String,
    pub cuda: Option<CudaHost>,
    pub rocm: Option<RocmHost>,
    pub vulkan: Option<VulkanHost>,
}

#[derive(Debug, Clone)]
pub struct CudaHost {
    /// Packed compute capability, e.g. 86 for `sm_86` (major=8, minor=6).
    pub compute_capability: u32,
    /// Installed CUDA major version (e.g. 12 or 13).
    pub runtime_major: u32,
    pub cudnn_present: bool,
}

/// The host's AMD compute capability.
#[derive(Debug, Clone)]
pub struct RocmHost {
    /// Every AMD GPU's architecture target, from KFD sysfs. Non-empty; the
    /// whole struct is `None` when the host has no AMD compute node.
    ///
    /// A `Vec` because `gpu_probe::detect()` reports every GPU, unlike
    /// `cuda_host()`, which documents itself as device 0 only.
    pub gfx_targets: Vec<gpu_probe::GfxTarget>,
    /// Installed `ROCm` userspace release, when one was found. Diagnostic
    /// only — see [`rocm_capability`].
    pub version: Option<gpu_probe::RocmVersion>,
}

/// The host's Vulkan runtime.
#[derive(Debug, Clone)]
pub struct VulkanHost {
    pub api_version: gpu_probe::VulkanVersion,
}

#[must_use]
pub fn detect() -> Host {
    let gpus = gpu_probe::detect();
    let gfx_targets: Vec<gpu_probe::GfxTarget> = gpus
        .iter()
        .filter_map(|g| g.arch_target.and_then(gpu_probe::ArchTarget::gfx))
        .collect();
    Host {
        target_triple: target_triple().into(),
        cuda: detect_cuda(),
        rocm: rocm_capability(&gfx_targets, gpu_probe::rocm_host().map(|h| h.version)),
        vulkan: gpu_probe::vulkan_host().map(|h| VulkanHost {
            api_version: h.api_version,
        }),
    }
}

/// Assemble the AMD capability record from the two independent facts about it.
///
/// The architecture targets decide. `gpu_probe::rocm_host()` reads
/// `$ROCM_PATH/.info/version` falling back to `/opt/rocm`, and its own
/// documentation is explicit that `None` is a weak negative: a distro
/// packaging `ROCm` into `/usr`, or a container carrying only the runtime
/// libraries, reports `None` while working. Subprocess backends compound this
/// — they typically bundle their runtime in the release tarball, so the host
/// needs no `ROCm` install at all.
///
/// Gating on the version would therefore refuse a working asset on a working
/// machine. The gfx target has no such problem: KFD publishes it from the
/// `amdgpu` kernel driver, and it is exactly what a HIP code object must be
/// built for. So the version rides along for logging and for the
/// incompatibility reason, and never decides anything.
///
/// Its own function because that asymmetry is the whole point and a caller
/// re-deriving it would get it wrong.
fn rocm_capability(
    gfx_targets: &[gpu_probe::GfxTarget],
    version: Option<gpu_probe::RocmVersion>,
) -> Option<RocmHost> {
    if gfx_targets.is_empty() {
        return None;
    }
    Some(RocmHost {
        gfx_targets: gfx_targets.to_vec(),
        version,
    })
}

/// Compile-time host triple. The daemon binary is built for one target,
/// so the host triple equals the build triple. Hard-fails to compile on
/// platforms the daemon doesn't yet support — intentional: that's a build-
/// time signal that someone needs to add an arm here.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn target_triple() -> &'static str {
    "x86_64-unknown-linux-gnu"
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn target_triple() -> &'static str {
    "aarch64-unknown-linux-gnu"
}

/// CUDA properties come from `gpu-probe`, which owns the single process-wide
/// NVML handle. Initializing NVML per call leaks a file descriptor each time,
/// so this must not open its own — that is why the daemon has no direct
/// `nvml-wrapper` dependency.
///
/// `gpu-probe` reports the parts separately and rejects the negative values a
/// misbehaving driver can return; the packed `sm_XX` form is this crate's
/// contract with `compat::select`, so it is assembled here.
fn detect_cuda() -> Option<CudaHost> {
    let host = gpu_probe::cuda_host()?;
    Some(CudaHost {
        compute_capability: pack_sm(host.compute_capability.major, host.compute_capability.minor),
        runtime_major: host.driver_version.major,
        cudnn_present: detect_cudnn(),
    })
}

/// Pack a major/minor compute capability into the `sm_XX` form
/// [`compat::select`](super::compat::select) matches an asset's `cuda_sm`
/// against — 8 and 6 become 86, the same integer `nvidia-smi` renders as `8.6`.
///
/// Its own function because a wrong packing here silently mis-selects every
/// CUDA asset rather than failing loudly.
const fn pack_sm(major: u32, minor: u32) -> u32 {
    major * 10 + minor
}

fn detect_cudnn() -> bool {
    use std::path::Path;
    for p in &[
        "/usr/lib/x86_64-linux-gnu/libcudnn.so",
        "/usr/lib64/libcudnn.so",
        "/usr/local/cuda/lib64/libcudnn.so",
    ] {
        if Path::new(p).exists() {
            return true;
        }
    }
    if let Ok(out) = std::process::Command::new("ldconfig").arg("-p").output()
        && String::from_utf8_lossy(&out.stdout).contains("libcudnn.so")
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{detect, pack_sm, rocm_capability};

    /// `compat::select` compares this packed integer against an asset's
    /// `cuda_sm`, so the encoding is a contract with the registry metadata, not
    /// an internal detail. A wrong packing mis-selects every CUDA asset
    /// silently — nothing downstream would reject an implausible value.
    #[test]
    fn compute_capability_packs_to_the_sm_form_assets_declare() {
        // An RTX 30-series host: nvidia-smi reports compute_cap 8.6, assets
        // declare cuda_sm = 86.
        assert_eq!(pack_sm(8, 6), 86);
        // Two-digit minors do not occur in CUDA's scheme, but a single-digit
        // minor must not lose its place: 9.0 is 90, never 9.
        assert_eq!(pack_sm(9, 0), 90);
        assert_eq!(pack_sm(12, 0), 120);
    }

    /// `rocm_host()` returning `None` is a weak negative — a distro shipping
    /// ROCm into `/usr`, or a backend bundling its own runtime, both work
    /// without `/opt/rocm`. The gfx target is the real signal: KFD publishes it
    /// from the kernel driver with no ROCm userspace installed at all. So the
    /// presence of gfx targets decides, and the version is carried only for
    /// diagnostics.
    #[test]
    fn rocm_capability_is_keyed_on_gfx_targets_not_the_userspace_version() {
        assert!(
            rocm_capability(&[], Some(gpu_probe::RocmVersion::new(6, 2, 4))).is_none(),
            "a ROCm install with no AMD compute node is not a usable host"
        );
        let host = rocm_capability(&[gpu_probe::GfxTarget::new(10, 3, 0)], None)
            .expect("gfx targets alone make the host usable");
        assert_eq!(host.gfx_targets, vec![gpu_probe::GfxTarget::new(10, 3, 0)]);
        assert!(host.version.is_none());
    }

    #[test]
    fn detect_never_panics_and_reports_this_host() {
        // Environment-dependent: asserts invariants, not specific hardware.
        let host = detect();
        assert!(!host.target_triple.is_empty());
        if let Some(rocm) = &host.rocm {
            assert!(
                !rocm.gfx_targets.is_empty(),
                "a ROCm host must carry targets"
            );
        }
    }
}
