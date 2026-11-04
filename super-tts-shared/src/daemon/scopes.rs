// SPDX-License-Identifier: GPL-3.0-only
//! Super Voice's auth scope catalog: the scopes every daemon understands
//! ([`super_engine_protocol::scopes::CORE_SCOPES`]) plus Super Voice's own,
//! for both features.
//! Shared so the daemon (which validates `/auth/request`) and the consent
//! dialog (which describes each scope to the user) can't drift. When they
//! drift, a scope the daemon accepts but the consent binary doesn't recognize
//! renders the "unknown scope — deny is safe" warning on a legitimate prompt,
//! teaching users to distrust real requests.

use super_engine_protocol::scopes;

use crate::SUPER_VOICE;

/// Every scope token the daemon understands, in wire (`snake_case`) form. A
/// token may be granted any non-empty subset. Source of truth for
/// `/auth/request` validation and the consent dialog; mirrors the scope
/// catalog in `docs/protocol/auth.md`.
pub fn known_scopes() -> impl Iterator<Item = &'static str> {
    scopes::known_scopes(&SUPER_VOICE)
}

/// True if `s` is a recognized scope token.
#[must_use]
pub fn is_known_scope(s: &str) -> bool {
    scopes::is_known_scope(&SUPER_VOICE, s)
}
