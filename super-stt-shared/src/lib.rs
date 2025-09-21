// SPDX-License-Identifier: GPL-3.0-only
pub mod auth;
#[cfg(test)]
mod auth_integration_test;
pub mod daemon;
pub mod models;
pub mod networking;
pub mod resource_management;
pub mod services;
pub mod utils;
pub mod validation;

#[cfg(feature = "analysis")]
pub mod audio;

// Re-export commonly used types for convenience
pub use auth::UdpAuth;
pub use models::*;
pub use networking::*;
pub use services::*;
pub use utils::logger;

#[cfg(feature = "audio")]
pub use utils::audio as audio_utils;

#[cfg(feature = "analysis")]
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
