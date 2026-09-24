// SPDX-License-Identifier: GPL-3.0-only
//! Resolve the effective transcription language for the active model. The
//! resolver and the published tags are `super_engine_daemon::language`'s.

pub use super_engine_daemon::language::{GLOBAL_LANGUAGES, is_offered_globally, resolve_language};
