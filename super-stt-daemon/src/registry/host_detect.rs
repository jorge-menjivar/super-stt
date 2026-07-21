// SPDX-License-Identifier: GPL-3.0-only
//! Detect the host's target triple, CUDA compute capability, runtime CUDA
//! major version, and cuDNN presence. Used by `compat::select` to pick a
//! compatible asset, and surfaced in the install failure error path.

#[derive(Debug, Clone)]
pub struct Host {
    pub target_triple: String,
    pub cuda: Option<CudaHost>,
}

#[derive(Debug, Clone)]
pub struct CudaHost {
    /// Packed compute capability, e.g. 86 for `sm_86` (major=8, minor=6).
    pub compute_capability: u32,
    /// Installed CUDA major version (e.g. 12 or 13).
    pub runtime_major: u32,
    pub cudnn_present: bool,
}

#[must_use]
pub fn detect() -> Host {
    Host {
        target_triple: target_triple().into(),
        cuda: detect_cuda(),
    }
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
    use super::pack_sm;

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
}
