// SPDX-License-Identifier: GPL-3.0-only
pub mod dbus;

// Re-export commonly used types
pub use dbus::DBusManager;
/// The served interface exists only where there is a bus to serve it on.
#[cfg(target_os = "linux")]
pub use dbus::SuperSTTDBusService;
