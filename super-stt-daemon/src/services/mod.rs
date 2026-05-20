// SPDX-License-Identifier: GPL-3.0-only
pub mod dbus;
pub mod transcription;

// Re-export commonly used types
pub use dbus::{DBusManager, SuperSTTDBusService};
pub use transcription::RealTimeTranscriptionManager;
