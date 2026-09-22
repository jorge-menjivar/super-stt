// SPDX-License-Identifier: GPL-3.0-only
//! `v1/settings` — the daemon's stored preferences, most of them a
//! `settings_getter!`/`settings_setter!` pair.

use serde_json::json;

use super::fake_daemon::{FakeDaemon, Reply};
use crate::daemon::client::v1::settings::{
    audio_theme, custom_models_dir, language, notification_method, preview_typing,
    recording_stop_mode, update_beta_optin, update_check_enabled, volume, write_method,
};
use crate::state::AudioTheme;

/// The generated getter reads one field out of the envelope; the generated
/// setter posts one key. Both are macro bodies shared by eight endpoints, so
/// one broken expansion would be eight broken settings — checked here against
/// the paths and keys the daemon actually serves.
#[tokio::test]
async fn a_generated_getter_reads_its_field_and_a_setter_posts_its_key() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(
            &json!({ "status": "success", "message": "Volume is 75" }),
        ))
        .reply(Reply::ok());

    let level = volume::get_volume().await.expect("volume read");
    volume::set_volume(20).await.expect("volume set");

    let requests = daemon.requests();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path(), "/settings/volume");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path(), "/settings/volume");
    assert_eq!(requests[1].json(), json!({ "volume": 20 }));
    assert_eq!(level, 75);
}

/// Each one-value setting carries its own key name, and the daemon reads only
/// that one. A generated setter posting the wrong key would be accepted and
/// then ignored — the setting silently never changing, with no error anywhere.
#[tokio::test]
async fn each_generated_setter_posts_the_key_its_endpoint_reads() {
    let daemon = FakeDaemon::start().await;

    write_method::set_write_method("ydotool".to_string())
        .await
        .expect("write method set");
    notification_method::set_notification_method("desktop".to_string())
        .await
        .expect("notification method set");
    recording_stop_mode::set_recording_stop_mode("manual_only".to_string())
        .await
        .expect("stop mode set");
    update_check_enabled::set_update_check_enabled(false)
        .await
        .expect("update check set");
    preview_typing::set_preview_typing(true)
        .await
        .expect("preview typing set");

    let sent: Vec<(String, serde_json::Value)> = daemon
        .requests()
        .iter()
        .map(|request| (request.path().to_string(), request.json()))
        .collect();
    assert_eq!(
        sent,
        vec![
            (
                "/settings/write_method".to_string(),
                json!({ "method": "ydotool" })
            ),
            (
                "/settings/notification_method".to_string(),
                json!({ "method": "desktop" })
            ),
            (
                "/settings/recording_stop_mode".to_string(),
                json!({ "mode": "manual_only" })
            ),
            (
                "/settings/update_check_enabled".to_string(),
                json!({ "enabled": false })
            ),
            (
                "/settings/preview_typing".to_string(),
                json!({ "enabled": true })
            ),
        ]
    );
}

/// A daemon older than a setting omits its field. Each getter then has to
/// answer with the value the daemon itself would have defaulted to, since the
/// UI renders the answer as the current setting either way.
#[tokio::test]
async fn a_getter_whose_field_is_absent_falls_back_to_the_daemons_default() {
    let daemon = FakeDaemon::start().await;
    for _ in 0..6 {
        daemon.reply(Reply::json(&json!({ "status": "success" })));
    }

    assert_eq!(volume::get_volume().await.expect("volume"), 100);
    assert_eq!(
        write_method::get_write_method()
            .await
            .expect("write method"),
        "auto"
    );
    assert_eq!(
        notification_method::get_notification_method()
            .await
            .expect("notification method"),
        "auto"
    );
    assert_eq!(
        recording_stop_mode::get_recording_stop_mode()
            .await
            .expect("stop mode"),
        "silence_and_manual"
    );
    assert!(
        update_check_enabled::get_update_check_enabled()
            .await
            .expect("update check"),
        "checking for updates is on unless the daemon says otherwise"
    );
    assert!(
        !preview_typing::get_preview_typing()
            .await
            .expect("preview typing")
    );
}

/// The override is `Option<Option<String>>` on the wire: absent means "the
/// daemon did not say", `null` means "no override set". Both render as the
/// default models directory, and neither is a failure.
#[tokio::test]
async fn an_unset_custom_models_dir_reads_as_none() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(
            &json!({ "status": "success", "custom_models_dir": null }),
        ))
        .reply(Reply::json(&json!({
            "status": "success",
            "custom_models_dir": "/home/me/models",
        })));

    assert_eq!(
        custom_models_dir::get_custom_models_dir()
            .await
            .expect("dir read"),
        None
    );
    assert_eq!(
        custom_models_dir::get_custom_models_dir()
            .await
            .expect("dir read"),
        Some("/home/me/models".to_string())
    );
    assert_eq!(
        daemon.requests()[0].path(),
        "/settings/custom_models_dir",
        "both reads hit the same path"
    );
}

/// The error a daemon reports in the envelope has to reach the caller as the
/// daemon's own words, through the generated body as much as a hand-written
/// one.
#[tokio::test]
async fn a_generated_setter_surfaces_the_daemons_refusal() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "error",
        "message": "ydotool is not installed",
    })));

    let error = write_method::set_write_method("ydotool".to_string())
        .await
        .expect_err("refused");

    assert_eq!(error.to_string(), "ydotool is not installed");
}

#[tokio::test]
async fn reading_and_writing_the_audio_theme() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(
            &json!({ "status": "success", "audio_theme": "scifi" }),
        ))
        .reply(Reply::json(
            &json!({ "status": "success", "message": "Theme set to Retro" }),
        ));

    let current = audio_theme::get_current_audio_theme()
        .await
        .expect("theme read");
    let confirmation = audio_theme::set_audio_theme(AudioTheme::Retro)
        .await
        .expect("theme set");

    let requests = daemon.requests();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path(), "/settings/audio_theme");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path(), "/settings/audio_theme");
    assert_eq!(requests[1].json(), json!({ "theme": "retro" }));
    assert_eq!(current, AudioTheme::SciFi);
    assert_eq!(confirmation, "Theme set to Retro");
}

/// A theme name this build does not know is the default, not an error: the
/// daemon owns the list, and a settings page that refused to load because of
/// one unrecognized string would be unusable against a newer daemon.
#[tokio::test]
async fn an_unknown_theme_name_reads_as_the_default() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(
        &json!({ "status": "success", "audio_theme": "from-a-later-daemon" }),
    ));

    let current = audio_theme::get_current_audio_theme()
        .await
        .expect("theme read");

    assert_eq!(current, AudioTheme::default());
}

/// Auditioning a theme is two requests — store it, then play it — so the
/// preview is of what is now configured rather than of a value that failed to
/// save.
#[tokio::test]
async fn auditioning_a_theme_stores_it_first() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(
            &json!({ "status": "success", "message": "stored" }),
        ))
        .reply(Reply::json(
            &json!({ "status": "success", "message": "played" }),
        ));

    let played = audio_theme::set_and_test_audio_theme(AudioTheme::Gentle)
        .await
        .expect("theme auditioned");

    let requests = daemon.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path(), "/settings/audio_theme");
    assert_eq!(requests[1].path(), "/settings/audio_theme/test");
    assert_eq!(played, "played");
}

#[tokio::test]
async fn a_theme_that_fails_to_store_is_never_played() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(
        &json!({ "status": "error", "message": "read-only config" }),
    ));

    let error = audio_theme::set_and_test_audio_theme(AudioTheme::Gentle)
        .await
        .expect_err("store failed");

    assert_eq!(error.to_string(), "read-only config");
    assert_eq!(daemon.requests().len(), 1);
}

/// The theme list falls back to the built-in set when the daemon cannot answer
/// — the picker has to offer something, and the built-ins are what this build
/// ships with anyway.
#[tokio::test]
async fn the_theme_list_falls_back_to_the_built_in_set() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(&json!({
            "status": "success",
            "available_audio_themes": ["classic", "silent"],
        })))
        .reply(Reply::status(500, &json!({ "status": "error" })));

    let from_daemon = audio_theme::load_audio_themes().await;
    let fallback = audio_theme::load_audio_themes().await;

    assert_eq!(daemon.requests()[0].path(), "/settings/audio_theme/list");
    assert_eq!(from_daemon, vec![AudioTheme::Classic, AudioTheme::Silent]);
    assert_eq!(fallback, AudioTheme::all_themes());
}

/// The global language is a string or `null` on the wire, and `null` means
/// "unset — auto-detect or use the model default", which is a value the UI
/// renders rather than a missing answer.
#[tokio::test]
async fn the_global_language_round_trips_and_clears() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(
            &json!({ "status": "success", "language": "es" }),
        ))
        .reply(Reply::ok())
        .reply(Reply::ok())
        .reply(Reply::json(
            &json!({ "status": "success", "language": null }),
        ));

    let set_to = language::get_primary_language()
        .await
        .expect("language read");
    language::set_primary_language("fr".to_string())
        .await
        .expect("language set");
    language::clear_primary_language()
        .await
        .expect("language cleared");
    let after_clear = language::get_primary_language()
        .await
        .expect("language read");

    let requests = daemon.requests();
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path(), "/settings/language");
    assert_eq!(requests[1].json(), json!({ "language": "fr" }));
    assert_eq!(requests[2].method, "DELETE");
    assert_eq!(requests[2].path(), "/settings/language");
    assert_eq!(set_to.as_deref(), Some("es"));
    assert_eq!(after_clear, None);
}

/// Which spellings the setter accepts — `en` or `en-US` — is a rule only the
/// daemon's resolver knows, so the vocabulary is asked for rather than curated
/// here.
#[tokio::test]
async fn the_languages_the_global_setting_accepts_come_from_the_daemon() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "available_languages": ["auto", "en", "es"],
    })));

    let listed = language::list_primary_languages()
        .await
        .expect("languages listed");

    assert_eq!(daemon.request().path(), "/settings/language/list");
    assert_eq!(listed, vec!["auto", "en", "es"]);
}

/// With `auto` configured, the resolved name is the only way to see which rung
/// of the chain typed. `auto` is the one value it can never be, so the daemon
/// echoing it back means "nothing to show" rather than a backend.
#[tokio::test]
async fn the_write_method_test_reports_the_backend_that_typed() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(&json!({
            "status": "success",
            "resolved_write_method": "ydotool",
        })))
        .reply(Reply::json(&json!({
            "status": "success",
            "resolved_write_method": "auto",
        })));

    let resolved = write_method::test_write_method().await.expect("typed");
    let unnameable = write_method::test_write_method().await.expect("typed");

    let requests = daemon.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path(), "/settings/write_method/test");
    assert_eq!(requests[0].json(), json!({}));
    assert_eq!(
        resolved,
        Some(super_stt_shared::models::write_method::WriteMethod::Ydotool)
    );
    assert_eq!(unnameable, None);
}

#[tokio::test]
async fn the_beta_optin_posts_its_value() {
    let daemon = FakeDaemon::start().await;

    update_beta_optin::set_update_beta_optin("enabled".to_string())
        .await
        .expect("opt-in set");

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/settings/update_beta_optin");
    assert_eq!(request.json(), json!({ "value": "enabled" }));
}
