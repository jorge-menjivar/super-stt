// SPDX-License-Identifier: GPL-3.0-only
pub mod daemon;
pub mod logging;
pub mod models;
pub mod paths;
pub mod registry;
pub mod utils;
pub mod validation;

pub mod audio;

// Re-export commonly used types for convenience
pub use models::*;

#[cfg(feature = "audio")]
pub use utils::audio as audio_utils;

pub use audio::*;

/// Macro to conditionally provide GPU device options based on CUDA feature availability
#[macro_export]
macro_rules! device_options {
    () => {{
        let mut devices = vec!["cpu".to_string()];
        #[cfg(feature = "cuda")]
        {
            devices.push("cuda".to_string());
        }
        devices
    }};
}

/// Check if CUDA support is available at compile time
#[macro_export]
macro_rules! has_cuda_support {
    () => {
        cfg!(feature = "cuda")
    };
}
