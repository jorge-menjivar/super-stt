// SPDX-License-Identifier: GPL-3.0-only
//! The two `LaunchAgent`s inside the macOS app bundle, and their registration
//! with launchd through `SMAppService`.
//!
//! The daemon and the shortcut listener run as `LaunchAgent`s whose plists
//! ship in `Super STT.app/Contents/Library/LaunchAgents/`. Nothing copies them
//! into `~/Library/LaunchAgents`: registering them through `ServiceManagement`
//! is what has launchd load them from the bundle, list them under System
//! Settings › General › Login Items as part of Super STT, and drop them when
//! the app is deleted.
//!
//! `SMAppService` finds a plist through the calling process's main bundle,
//! so these calls mean something only from an executable inside the bundle —
//! the settings app, or `Contents/MacOS/super-stt-cli`. From a bare binary in
//! `target/` every agent is [`Status::NotInBundle`].

use anyhow::{Result, anyhow};
use objc2::rc::Retained;
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_service_management::{
    SMAppService, SMAppServiceStatus, kSMErrorAlreadyRegistered, kSMErrorJobNotFound,
};
use std::path::PathBuf;

/// One of the bundle's `LaunchAgent`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    /// `super-stt-daemon`.
    Daemon,
    /// `super-stt-cli hotkey`, the global shortcut.
    Hotkey,
}

/// What launchd and the Login Items settings say about an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Never registered, or unregistered since.
    NotRegistered,
    /// Registered and allowed to run.
    Enabled,
    /// Registered, but switched off in Login Items. Only the user can turn it
    /// back on; registering again does not.
    RequiresApproval,
    /// The running executable is not inside a bundle that carries this plist.
    NotInBundle,
}

impl Agent {
    pub const ALL: [Self; 2] = [Self::Daemon, Self::Hotkey];

    /// The launchd label, which is also the plist's file stem.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Daemon => "ai.menjivar.super-stt",
            Self::Hotkey => "ai.menjivar.super-stt.hotkey",
        }
    }

    fn plist_name(self) -> String {
        format!("{}.plist", self.label())
    }

    fn service(self) -> Retained<SMAppService> {
        let plist = NSString::from_str(&self.plist_name());
        // SAFETY: a class constructor that only reads the name it is given.
        unsafe { SMAppService::agentServiceWithPlistName(&plist) }
    }

    /// Whether the plist is where `SMAppService` looks for it.
    ///
    /// Checked here rather than read off [`SMAppServiceStatus::NotFound`],
    /// which does not mean that: it is what macOS reports for any agent it
    /// has no record of, which includes every agent before its first
    /// registration. (Background Task Management logs it as "record not
    /// found".)
    fn in_bundle(self) -> bool {
        let bundle = PathBuf::from(NSBundle::mainBundle().bundlePath().to_string());
        bundle
            .join("Contents/Library/LaunchAgents")
            .join(self.plist_name())
            .is_file()
    }

    #[must_use]
    pub fn status(self) -> Status {
        if !self.in_bundle() {
            return Status::NotInBundle;
        }
        // SAFETY: a property read on a live service object.
        match unsafe { self.service().status() } {
            SMAppServiceStatus::Enabled => Status::Enabled,
            SMAppServiceStatus::RequiresApproval => Status::RequiresApproval,
            _ => Status::NotRegistered,
        }
    }

    /// Register the agent, which also starts it. An agent that is already
    /// registered stays as it is.
    ///
    /// Asked of `ServiceManagement` every time, not skipped when
    /// [`Self::status`] says the agent is enabled. The first install over
    /// the agents from before the bundle, which had the same labels, found
    /// both "enabled" minutes before macOS had any record of either, and
    /// skipping on that left nothing loaded.
    ///
    /// # Errors
    /// When this executable is not inside the app bundle, or when
    /// `ServiceManagement` refuses, with its own description of why.
    pub fn register(self) -> Result<()> {
        if !self.in_bundle() {
            return Err(self.not_in_bundle());
        }
        // SAFETY: a method call on a live service object.
        match unsafe { self.service().registerAndReturnError() } {
            Ok(()) => Ok(()),
            Err(e) if code_is(&e, kSMErrorAlreadyRegistered) => Ok(()),
            Err(e) => Err(self.failure("register", &e)),
        }
    }

    /// Unregister the agent, which also stops it. An agent that is not
    /// registered stays as it is.
    ///
    /// # Errors
    /// As [`Self::register`].
    pub fn unregister(self) -> Result<()> {
        if !self.in_bundle() {
            return Err(self.not_in_bundle());
        }
        // SAFETY: a method call on a live service object.
        match unsafe { self.service().unregisterAndReturnError() } {
            Ok(()) => Ok(()),
            Err(e) if code_is(&e, kSMErrorJobNotFound) => Ok(()),
            Err(e) => Err(self.failure("unregister", &e)),
        }
    }

    fn not_in_bundle(self) -> anyhow::Error {
        anyhow!(
            "{} is not in this executable's app bundle; run the copy inside \
             Super STT.app/Contents/MacOS",
            self.plist_name()
        )
    }

    fn failure(self, verb: &str, error: &NSError) -> anyhow::Error {
        anyhow!(
            "could not {verb} {}: {} (code {})",
            self.label(),
            error.localizedDescription(),
            error.code()
        )
    }
}

fn code_is(error: &NSError, code: std::ffi::c_uint) -> bool {
    isize::try_from(code).is_ok_and(|code| error.code() == code)
}

/// Open System Settings at General › Login Items, the one place an agent in
/// [`Status::RequiresApproval`] can be switched back on.
pub fn open_login_items_settings() {
    // SAFETY: a class method with no arguments.
    unsafe { SMAppService::openSystemSettingsLoginItems() };
}
