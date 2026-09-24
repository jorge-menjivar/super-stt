// SPDX-License-Identifier: GPL-3.0-only
//! The self-healing `/events` subscription the widgets run on:
//! [`super_engine_client::widget_subscription`], with the topic table bound to
//! Super STT's.

pub use super_engine_client::widget_subscription::*;
use super_engine_protocol::scopes;

use crate::SUPER_STT;

/// The scope a subscriber needs for Super STT's event `topic`, or `None` for
/// a topic Super STT does not publish. Mirrors the daemon's
/// `Topic::required_scope` (a daemon-side test pins the two together) and the
/// topic tables in `docs/protocol/endpoints/v1/events.md`.
#[must_use]
pub fn required_scope_for_topic(topic: &str) -> Option<&'static str> {
    scopes::required_scope_for_topic(&SUPER_STT, topic)
}

/// The first topic in `topics` that `scopes` does not grant (or whose name is
/// unknown), or `None` when every topic is covered. Clients assert this in
/// their tests so a topic added without its scope fails CI rather than
/// silently 403-ing the whole stream at runtime.
#[must_use]
pub fn uncovered_topic<'t>(granted: &[&str], topics: &[&'t str]) -> Option<&'t str> {
    scopes::uncovered_topic(&SUPER_STT, granted, topics)
}
