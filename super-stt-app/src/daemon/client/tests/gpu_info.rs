// SPDX-License-Identifier: GPL-3.0-only
//! `v1/gpu_info` — the host's GPU inventory.

use serde_json::json;

use super::fake_daemon::{FakeDaemon, Reply};
use crate::daemon::client::v1::gpu_info;

#[tokio::test]
async fn the_inventory_comes_back_typed() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "gpu_info": [{
            "name": "NVIDIA GeForce RTX 4090",
            "vendor": "nvidia",
            "total_bytes": 25_757_220_864_u64,
            "free_bytes": 24_000_000_000_u64,
            "arch_target": "sm_89",
        }],
    })));

    let gpus = gpu_info::get_gpu_info().await.expect("inventory read");

    let request = daemon.request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/gpu_info");
    assert_eq!(gpus.len(), 1);
    assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4090");
    assert_eq!(gpus[0].arch_target.as_deref(), Some("sm_89"));
}

/// A machine with no GPU is the ordinary case, not a failure: the daemon omits
/// the field and the memory readouts simply do not render.
#[tokio::test]
async fn a_host_with_no_gpu_reports_an_empty_inventory() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({ "status": "success" })));

    let gpus = gpu_info::get_gpu_info().await.expect("inventory read");

    assert!(gpus.is_empty());
}

/// A GPU whose driver reports no architecture — an Intel or Apple part, or an
/// AMD card on a kernel without KFD — still lists, with the field null. Losing
/// the whole inventory over it would hide the memory readout the picker needs.
#[tokio::test]
async fn a_gpu_without_an_arch_target_still_lists() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "gpu_info": [{
            "name": "Intel Arc A770",
            "vendor": "intel",
            "total_bytes": 17_179_869_184_u64,
            "arch_target": null,
        }],
    })));

    let gpus = gpu_info::get_gpu_info().await.expect("inventory read");

    assert_eq!(gpus.len(), 1);
    assert!(gpus[0].arch_target.is_none());
    assert!(gpus[0].free_bytes.is_none());
}

#[tokio::test]
async fn a_probe_the_daemon_could_not_run_is_an_error() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "error",
        "message": "gpu probe unavailable",
    })));

    let error = gpu_info::get_gpu_info().await.expect_err("probe failed");

    assert_eq!(error.to_string(), "gpu probe unavailable");
}
