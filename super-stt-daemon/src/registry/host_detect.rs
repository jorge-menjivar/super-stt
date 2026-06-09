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

fn detect_cuda() -> Option<CudaHost> {
    let nvml = nvml_wrapper::Nvml::init().ok()?;
    let dev = nvml.device_by_index(0).ok()?;

    // nvml-wrapper 0.12 returns `CudaComputeCapability { major: i32, minor: i32 }`.
    let cc = dev.cuda_compute_capability().ok()?;
    // Guard against negative values from a misbehaving driver.
    if cc.major < 0 || cc.minor < 0 {
        return None;
    }
    let cc_packed = cc.major.cast_unsigned() * 10 + cc.minor.cast_unsigned();

    // `sys_cuda_driver_version()` returns a packed `i32`, e.g. 12090 for CUDA 12.9.
    let cuda_version = nvml.sys_cuda_driver_version().ok()?;
    if cuda_version <= 0 {
        return None;
    }
    let runtime_major = (cuda_version / 1000).cast_unsigned();

    let cudnn_present = detect_cudnn();
    Some(CudaHost {
        compute_capability: cc_packed,
        runtime_major,
        cudnn_present,
    })
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
