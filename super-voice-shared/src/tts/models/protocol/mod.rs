// SPDX-License-Identifier: GPL-3.0-only
mod command;
mod daemon_status;
mod dispatch;
mod error_code;
mod pipeline;
mod request;
mod response;

#[cfg(test)]
mod tests;

pub use command::Command;
pub use daemon_status::DaemonStatusEvent;
pub use error_code::ErrorCode;
pub use pipeline::{
    SYNTHESIS_STAGE, StageModelDevice, StageModelReport, StageReport, StageRole, StageSwitch,
    SwitchDownload, SwitchTarget, default_stage,
};
pub use request::DaemonRequest;
pub use response::{
    CudaHostInfo, DaemonResponse, DownloadProgress, GpuHostInfo, GpuInfo, NotificationEvent,
    RocmHostInfo, VulkanHostInfo,
};
/// A backend's own account of its load, and the ids it is worded in. The
/// same for every product built on super-engine.
pub use super_engine_protocol::models::load_progress::{self, LoadProgress};
