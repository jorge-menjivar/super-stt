// SPDX-License-Identifier: GPL-3.0-only
settings_setter!(
    set_write_method,
    SetWriteMethodBody { method: String },
    "set_write_method",
    "method"
);
settings_dispatch!(get_write_method, "get_write_method");
