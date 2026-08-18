// SPDX-License-Identifier: GPL-3.0-only
//! `/active_device` — compute device (CPU/CUDA). GPU memory is served
//! separately by `GET /gpu_info` (gpu-probe).

settings_getter!(
    get_current_device -> (String, Vec<String>), "/active_device", "get_device",
    |resp| (
        resolve_current_device(resp.device, resp.resolved_accel.flatten()),
        resp.available_devices
            .unwrap_or_else(|| vec!["cpu".to_string()]),
    )
);
settings_setter!(set_device, device: String, "/active_device", "device", "set_device");

/// The device string seeded into `app.current_device` — rendered as the
/// active model card's `"· device"` suffix. `device` is only the user's
/// `cpu`/`gpu` *preference* and can lie: a `gpu` preference that silently
/// fell back to CPU still reads `device: "gpu"`. `resolved_accel` reports
/// what actually loaded (`cuda`/`rocm`/`metal`/`vulkan`/`cpu`), so it takes
/// priority; the preference is only a fallback for an older daemon that
/// omits the field, or before a `gpu` preference has resolved to anything
/// (`resolved_accel: null`, already flattened away by the caller).
fn resolve_current_device(device: Option<String>, resolved_accel: Option<String>) -> String {
    resolved_accel
        .or(device)
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::resolve_current_device;

    /// The reported defect: a `gpu` preference that fell back to CPU must not
    /// read as `"gpu"` on the active card. `resolved_accel` reports what
    /// actually loaded and wins over the preference.
    #[test]
    fn prefers_the_resolved_accelerator_over_the_preference() {
        assert_eq!(
            resolve_current_device(Some("gpu".to_string()), Some("cpu".to_string())),
            "cpu"
        );
        assert_eq!(
            resolve_current_device(Some("gpu".to_string()), Some("cuda".to_string())),
            "cuda"
        );
    }

    /// A `gpu` preference with nothing loaded yet resolves to `null`
    /// (flattened to `None` by the caller) — the preference is the only
    /// available answer.
    #[test]
    fn falls_back_to_the_preference_when_unresolved() {
        assert_eq!(resolve_current_device(Some("gpu".to_string()), None), "gpu");
    }

    #[test]
    fn falls_back_to_unknown_when_nothing_is_present() {
        assert_eq!(resolve_current_device(None, None), "unknown");
    }
}
