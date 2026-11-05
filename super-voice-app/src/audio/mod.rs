// SPDX-License-Identifier: GPL-3.0-only
//! The app's only audio surface: capturing a reference recording for a cloned
//! voice.
//!
//! Everything else the app does with sound goes through the daemon — it owns
//! the output device, and the app asks it to speak. Recording is the one thing
//! the daemon cannot do on the app's behalf: the microphone is opened for as
//! long as the user holds the button and for nothing else, which is a property
//! worth keeping visible in the process the user is looking at.

pub mod recorder;
