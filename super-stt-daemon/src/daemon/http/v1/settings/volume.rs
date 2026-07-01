// SPDX-License-Identifier: GPL-3.0-only
settings_setter!(
    set_volume,
    SetVolumeBody { volume: u8 },
    "set_volume",
    "volume"
);
settings_dispatch!(get_volume, "get_volume");
