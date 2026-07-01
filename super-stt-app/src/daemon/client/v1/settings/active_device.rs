// SPDX-License-Identifier: GPL-3.0-only
//! `/active_device` — compute device (CPU/CUDA). GPU memory is served
//! separately by `GET /gpu_info` (gpu-probe).

settings_getter!(
    get_current_device -> (String, Vec<String>), "/active_device", "get_device",
    |resp| (
        resp.device.unwrap_or_else(|| "unknown".to_string()),
        resp.available_devices
            .unwrap_or_else(|| vec!["cpu".to_string()]),
    )
);
settings_setter!(set_device, device: String, "/active_device", "device", "set_device");
