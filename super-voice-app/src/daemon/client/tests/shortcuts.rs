// SPDX-License-Identifier: GPL-3.0-only
//! `v1/shortcuts` — the Shortcuts page's three calls.

use serde_json::json;
use super_voice_shared::models::shortcuts::{ShortcutId, ShortcutMethod, ShortcutState};

use super::fake_daemon::{FakeDaemon, Reply};
use crate::daemon::client::v1::shortcuts::{add_shortcut, list_shortcuts, remove_shortcut};

/// What the daemon answers each of them with.
fn list(read_aloud: &str) -> serde_json::Value {
    json!({
        "status": "success",
        "method": "cosmic",
        "shortcuts": [
            { "id": "dictate", "state": "installed", "keys": "Super+Space", "command": "voice record --write" },
            { "id": "read_aloud", "state": read_aloud, "keys": "Super+Shift+Space", "command": "voice speak --read" },
        ],
    })
}

/// Each call goes to its path with its method, and reads the list back.
#[tokio::test]
async fn each_call_reaches_its_route_and_reads_the_list() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&list("missing")));
    daemon.reply(Reply::json(&list("installed")));
    daemon.reply(Reply::json(&list("missing")));

    let listed = list_shortcuts().await.expect("listed");
    assert_eq!(listed.method, ShortcutMethod::Cosmic);
    assert_eq!(listed.shortcuts[1].state, ShortcutState::Missing);

    let added = add_shortcut(ShortcutId::ReadAloud).await.expect("added");
    assert_eq!(added.shortcuts[1].state, ShortcutState::Installed);

    let removed = remove_shortcut(ShortcutId::ReadAloud)
        .await
        .expect("removed");
    assert_eq!(removed.shortcuts[1].state, ShortcutState::Missing);

    let sent: Vec<(String, String)> = daemon
        .requests()
        .iter()
        .map(|r| (r.method.clone(), r.path().to_string()))
        .collect();
    assert_eq!(
        sent,
        [
            ("GET".to_string(), "/shortcuts".to_string()),
            ("POST".to_string(), "/shortcuts/read_aloud".to_string()),
            ("DELETE".to_string(), "/shortcuts/read_aloud".to_string()),
        ]
    );
}
