// SPDX-License-Identifier: GPL-3.0-only
//! `/gpu_info` — what the daemon can see of this machine's accelerators.
//!
//! Contract: `docs/protocol/endpoints/v1/gpu_info.md`.
//!
//! The hardware inventory, independent of any model. What a *particular* model
//! can be run on here is narrower — the intersection of this and the builds the
//! model ships — and is answered by
//! [`/pipeline/{stage}/model/{model}/device/list`](super::pipeline::device).

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::Response;

use crate::daemon::http::v1::wire::{FromDaemon, GpuInventory};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};

#[utoipa::path(
    get,
    path = "/gpu_info",
    tag = "settings",
    summary = "Inventory the host's GPUs",
    description = "\
What the daemon can see of this machine's accelerators: one entry per detected GPU \
with its memory, plus the host-wide driver and runtime versions that decide which \
backend builds will actually run here.

Detection is a live probe, so this reflects the machine now rather than a cached \
answer. A host with no GPU answers `200` with an empty list.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The detected GPUs and host toolchain versions.", body = GpuInventory),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn get_gpu_info(State(s): State<AppState>) -> Response {
    let resp = dispatch(&s.daemon, build_request("get_gpu_info", None)).await;
    narrowed(resp, GpuInventory::from_daemon)
}
