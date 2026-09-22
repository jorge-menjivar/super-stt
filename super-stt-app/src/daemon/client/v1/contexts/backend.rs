// SPDX-License-Identifier: GPL-3.0-only
//! `/backend/{source}/context` — which dictation context one backend uses.
//!
//! Three states, three verbs, so nothing on the wire is a magic value:
//! [`clear_backend_context`] puts the backend back on the active context,
//! [`set_backend_context`] with an id pins it, and the same call with `None`
//! sends it no context at all.
//!
//! Filed under [`super`] rather than beside the other `/backend/` calls because
//! the subject is contexts — which is also how the daemon tags it.

use crate::daemon::client::internal::session::with_settings_token;
use serde::Deserialize;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;
use super_stt_shared::models::contexts::DictationContext;

/// Which of the three a backend is in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    /// Follows whichever context is active. The default, and what almost every
    /// backend should be.
    #[default]
    Active,
    /// Uses [`BackendContext::id`], whatever is active.
    Pinned,
    /// Is sent no context at all.
    ///
    /// Spelled `Nothing` rather than `None` so a `match` on this never reads
    /// like one on an `Option`.
    #[serde(rename = "none")]
    Nothing,
}

/// What one backend uses, and what that resolves to.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BackendContext {
    #[serde(default)]
    pub mode: ContextMode,
    /// The id this backend is pinned to. Only meaningful when `mode` is
    /// [`ContextMode::Pinned`].
    #[serde(default)]
    pub id: Option<String>,
    /// What this backend will actually be sent, already resolved. `None` when
    /// nothing resolves — including a pin to a context that has since been
    /// deleted, which falls back to nothing rather than to the active one.
    #[serde(default)]
    pub context: Option<DictationContext>,
}

fn path(source: &str) -> String {
    format!("/backend/{}/context", urlencoding::encode(source))
}

/// `GET /backend/{source}/context`.
pub async fn get_backend_context(source: String) -> HttpResult<BackendContext> {
    with_settings_token(move |socket, token| {
        let source = source.clone();
        async move { transport::get_json::<BackendContext>(socket, &token, &path(&source)).await }
    })
    .await
}

/// `POST /backend/{source}/context` — pin this backend to `id`, or send it no
/// context at all with `None`.
///
/// `None` here is *not* "follow the active context" — that is
/// [`clear_backend_context`]. The distinction is the whole reason both exist.
pub async fn set_backend_context(source: String, id: Option<String>) -> HttpResult<BackendContext> {
    with_settings_token(move |socket, token| {
        let (source, id) = (source.clone(), id.clone());
        async move {
            let body = serde_json::json!({ "id": id });
            transport::post_json::<BackendContext>(socket, &token, &path(&source), &body).await
        }
    })
    .await
}

/// `DELETE /backend/{source}/context` — back to following the active context.
pub async fn clear_backend_context(source: String) -> HttpResult<BackendContext> {
    with_settings_token(move |socket, token| {
        let source = source.clone();
        async move {
            transport::delete_json::<BackendContext>(socket, &token, &path(&source)).await
        }
    })
    .await
}
