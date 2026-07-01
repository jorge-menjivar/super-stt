// SPDX-License-Identifier: GPL-3.0-only
settings_setter!(
    set_active_device,
    SetActiveDeviceBody { device: String },
    "set_device",
    "device"
);
settings_dispatch!(get_active_device, "get_device");
