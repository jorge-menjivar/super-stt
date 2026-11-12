// SPDX-License-Identifier: GPL-3.0-only
//! `/shortcuts` — Super Voice's keyboard shortcuts on this desktop. The daemon
//! owns them, so the app never writes desktop settings itself.

use crate::daemon::client::internal::session::with_settings_token;
use super_voice_shared::daemon::http_client::{HttpResult, transport};
use super_voice_shared::models::shortcuts::{ShortcutId, ShortcutList};

/// `GET /shortcuts` — every shortcut, where it stands, and the method.
pub async fn list_shortcuts() -> HttpResult<ShortcutList> {
    with_settings_token(|socket, token| async move {
        transport::get_json(socket, &token, "/shortcuts").await
    })
    .await
}

/// `POST /shortcuts/{id}` — add the shortcut. Answers the list.
pub async fn add_shortcut(id: ShortcutId) -> HttpResult<ShortcutList> {
    let path = format!("/shortcuts/{id}");
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move { transport::post_json(socket, &token, &path, &serde_json::json!({})).await }
    })
    .await
}

/// `DELETE /shortcuts/{id}` — remove the shortcut. Answers the list.
pub async fn remove_shortcut(id: ShortcutId) -> HttpResult<ShortcutList> {
    let path = format!("/shortcuts/{id}");
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move { transport::delete_json(socket, &token, &path).await }
    })
    .await
}
