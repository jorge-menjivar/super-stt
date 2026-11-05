// SPDX-License-Identifier: GPL-3.0-only
use super::*;

/// A config as this daemon stores it. Fails the test if it would have been
/// reset.
fn from_ron(content: &str) -> DaemonConfig {
    let (config, was_reset) = DaemonConfig::parse_or_reset(content);
    assert!(!was_reset, "the config must parse, not reset");
    config
}

/// `config` as the daemon writes it.
fn written(config: &DaemonConfig) -> String {
    super_engine_daemon::config::to_string(config).expect("serialize")
}

/// `config` after a trip through the file.
fn round_trip(config: &DaemonConfig) -> DaemonConfig {
    from_ron(&written(config))
}

/// A config naming only some sections loads, and every other section, and
/// every field left out of a named one, is its default. A setting added in a
/// later release is a field an earlier file does not have.
#[test]
fn a_config_with_only_some_sections_loads_the_rest_as_defaults() {
    let config = from_ron(
        r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    transcription: (preferred_model: "whisper-tiny"),
)"#,
    );
    assert_eq!(config.transcription.preferred_model, "whisper-tiny");
    assert!(config.transcription.active_backend.is_none());
    assert_eq!(config.notification_method, NotificationMethod::Auto);
    assert!(!config.post_processor.is_active());
    assert!(config.backends.options.is_empty());
    assert!(config.backends.context_override.is_empty());
    assert!(config.update.check_enabled);
    assert!(config.contexts.items.is_empty());
    assert!(config.active_context().is_none());
    assert!(config.http.tcp.enabled);
    assert!(config.http.tcp.admits_any_origin());
    assert_eq!(
        config.http.tcp.bind_addr(),
        DaemonConfig::default().http.tcp.bind_addr(),
        "a config without the section listens exactly where a fresh one does"
    );
}

#[test]
fn config_with_online_model_preferred_round_trips() {
    let mut config = DaemonConfig::default();
    config.transcription.preferred_model = "whisper-1".to_string();

    let parsed = round_trip(&config);
    assert_eq!(parsed.transcription.preferred_model, "whisper-1");
}

#[test]
fn config_preserves_all_online_model_variants() {
    for name in [
        "whisper-1",
        "gpt-4o-transcribe",
        "gpt-4o-mini-transcribe",
        "voxtral-mini-transcribe-v2",
        "nova-3",
    ] {
        let model = name.to_string();
        let mut config = DaemonConfig::default();
        config.transcription.preferred_model = model.clone();

        let parsed = round_trip(&config);
        assert_eq!(parsed.transcription.preferred_model, model);
    }
}

#[test]
fn backend_options_set_clear_and_round_trip() {
    let mut config = DaemonConfig::default();
    let src = "github.com/super-stt/openai";

    // Default: no override.
    assert_eq!(config.backend_option(src, "base_url"), None);

    // Set an override (no disk write needed for the in-memory assertions;
    // update_backend_option also persists, which is a no-op-safe save).
    config
        .backends
        .options
        .entry(src.to_string())
        .or_default()
        .insert("base_url".to_string(), "https://gw.example".to_string());
    assert_eq!(
        config.backend_option(src, "base_url"),
        Some("https://gw.example")
    );

    // Survives the file.
    let parsed = round_trip(&config);
    assert_eq!(
        parsed.backend_option(src, "base_url"),
        Some("https://gw.example")
    );

    // Empty value clears it and prunes the now-empty source map.
    let mut parsed = parsed;
    if let Some(opts) = parsed.backends.options.get_mut(src) {
        opts.remove("base_url");
        if opts.is_empty() {
            parsed.backends.options.remove(src);
        }
    }
    assert_eq!(parsed.backend_option(src, "base_url"), None);
    assert!(parsed.backends.options.is_empty());
}

/// `transcription.active_backend` defaults to `None` (no backend selected
/// at install time → daemon idle).
#[test]
fn active_backend_default_is_none() {
    let config = DaemonConfig::default();
    assert!(config.transcription.active_backend.is_none());
}

/// A persisted relative dir round-trips through the file, preserving the
/// stable handle the daemon uses to find the backend on restart.
#[test]
fn active_backend_round_trips_through_the_file() {
    let mut config = DaemonConfig::default();
    config.transcription.active_backend = Some("mistral".to_string());

    let serialized = written(&config);
    assert!(
        serialized.contains("active_backend: Some(\"mistral\")"),
        "serialized form: {serialized}"
    );

    let parsed = from_ron(&serialized);
    assert_eq!(
        parsed.transcription.active_backend.as_deref(),
        Some("mistral")
    );
}

#[test]
fn daemon_bad_theme_falls_back_preserving_rest() {
    // A single unrecognized enum value must NOT wipe the whole config.
    let cfg = from_ron(
        r#"(
    device: (preferred_device: "gpu"),
    audio: (theme: "Nonexistent", volume: 80),
    transcription: (
        preferred_model: "WhisperTiny",
        write_mode: true,
        recording_stop_mode: "manual_only",
        write_method: "ydotool",
    ),
)"#,
    );
    assert_eq!(cfg.audio.theme, AudioTheme::default()); // bad field reset
    assert_eq!(cfg.audio.volume, 80); // everything else preserved
    assert_eq!(cfg.device.preferred_device, "gpu");
    assert!(cfg.transcription.write_mode);
    assert_eq!(
        cfg.transcription.recording_stop_mode,
        RecordingStopMode::ManualOnly
    );
    assert_eq!(cfg.transcription.write_method, WriteMethod::Ydotool);
}

#[test]
fn daemon_bad_stop_mode_and_write_method_fall_back_preserving_rest() {
    let cfg = from_ron(
        r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "gentle", volume: 100),
    transcription: (
        preferred_model: "WhisperTiny",
        write_mode: false,
        recording_stop_mode: "BogusMode",
        write_method: "BogusMethod",
    ),
)"#,
    );
    assert_eq!(cfg.audio.theme, AudioTheme::Gentle); // preserved
    assert_eq!(
        cfg.transcription.recording_stop_mode,
        RecordingStopMode::default()
    );
    assert_eq!(cfg.transcription.write_method, WriteMethod::default());
}

#[test]
fn corrupt_daemon_config_resets_to_default() {
    let (cfg, was_reset) = DaemonConfig::parse_or_reset("this is not ::: valid ron [");
    assert!(was_reset, "garbage input must trigger a reset");
    // The reset config is a valid default (proves no panic, app can start).
    assert_eq!(cfg.audio.theme, AudioTheme::default());
    assert_eq!(cfg.device.preferred_device, "cpu");
}

/// A wire setter rejects an unparseable device outright; a persisted config
/// value has no such option — a daemon that refused to start over a stale
/// `preferred_device` would be worse than one that falls back to the default.
/// The deprecated `cuda`/`metal` spellings normalize to `gpu` rather than
/// falling back, so a config written before this vocabulary still loads onto
/// the accelerator it always meant.
#[test]
fn a_persisted_device_preference_normalizes_and_a_bogus_one_falls_back_to_cpu() {
    let ron_str = |device: &str| {
        format!(
            "(device: (preferred_device: \"{device}\"), \
             audio: (theme: \"classic\", volume: 100), \
             transcription: (preferred_model: \"\", write_mode: false))"
        )
    };

    let (cfg, was_reset) = DaemonConfig::parse_or_reset(&ron_str("cuda"));
    assert!(!was_reset);
    assert_eq!(cfg.device.preferred_device, "gpu");

    let (cfg, was_reset) = DaemonConfig::parse_or_reset(&ron_str("gpu"));
    assert!(!was_reset);
    assert_eq!(cfg.device.preferred_device, "gpu");

    // A garbage value (or `none`, a model sentinel rather than a preference)
    // degrades to the default instead of failing the whole config load.
    let (cfg, was_reset) = DaemonConfig::parse_or_reset(&ron_str("xpu"));
    assert!(
        !was_reset,
        "a stale device field must not reset the whole config"
    );
    assert_eq!(cfg.device.preferred_device, "cpu");

    let (cfg, was_reset) = DaemonConfig::parse_or_reset(&ron_str("none"));
    assert!(!was_reset);
    assert_eq!(cfg.device.preferred_device, "cpu");
}

/// The invariant behind the unload path (`disable_transcription`): unloading
/// switches the stage off and keeps everything else — the model, its source,
/// the backend — and that state must survive a save/reload.
///
/// Both halves matter, and they used to be in tension. A restart must not
/// reload the model the user just stopped, which is why the unload once erased
/// `preferred_model`; but erasing it also lost the selection, so the card came
/// back empty. The flag separates the two, exactly as `post_processor.enabled`
/// has always done for stage 2.
#[test]
fn an_unload_persists_as_switched_off_with_the_selection_kept() {
    let mut config = DaemonConfig::default();
    config.update_preferred_model("whisper-large-v3".to_string(), "openai-whisper".to_string());
    config.transcription.active_backend = Some("openai-whisper".to_string());
    assert!(config.transcription.is_active(), "a loaded stage is active");

    config.disable_transcription();

    let (parsed, was_reset) = DaemonConfig::parse_or_reset(&written(&config));
    assert!(!was_reset, "the config must re-parse cleanly");
    assert!(
        !parsed.transcription.is_active(),
        "restart must not reload an unloaded model"
    );
    assert_eq!(
        parsed.transcription.preferred_model, "whisper-large-v3",
        "the selection survives, so the card can offer to load it again"
    );
    assert_eq!(parsed.transcription.preferred_source, "openai-whisper");
    assert_eq!(
        parsed.transcription.active_backend.as_deref(),
        Some("openai-whisper"),
        "unload keeps the active backend selected"
    );
}

/// Both stages decide "should this be running?" the same way: switched on, and
/// pointed at a complete `(model, source)` pair.
///
/// A stage switched on with nothing selected is not running — and a card told
/// otherwise would offer to unload a model that does not exist.
#[test]
fn a_stage_with_no_selection_is_not_active_at_either_position() {
    let mut config = DaemonConfig::default();
    config.transcription.enabled = true;
    config.post_processor.enabled = true;
    assert!(!config.transcription.is_active());
    assert!(!config.post_processor.is_active());

    // Half a selection is no selection: a name without the backend serving it
    // resolves to nothing.
    config.transcription.preferred_model = "whisper-tiny".to_string();
    config.post_processor.model = "cleanup".to_string();
    assert!(!config.transcription.is_active());
    assert!(!config.post_processor.is_active());

    config.transcription.preferred_source = "github.com/super-stt/openai".to_string();
    config.post_processor.source = "github.com/super-stt/openai".to_string();
    assert!(config.transcription.is_active());
    assert!(config.post_processor.is_active());
}

#[test]
fn all_published_daemon_configs_load_cleanly() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/configs");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures/configs dir must exist") {
        let version_dir = entry.expect("readable dir entry").path();
        if !version_dir.is_dir() {
            continue; // skip README.md and any other non-version files
        }
        let fixture = version_dir.join("daemon.ron");
        let content = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|e| panic!("read {}: {e}", fixture.display()));
        let (_, was_reset) = DaemonConfig::parse_or_reset(&content);
        assert!(
            !was_reset,
            "daemon fixture {} must load cleanly (no reset)",
            fixture.display()
        );
        checked += 1;
    }
    // Super Voice started fresh and has no release yet; each one adds the
    // `daemon.ron` it writes. Until the first, there is nothing to check.
    eprintln!("checked {checked} published daemon configs");
}

/// A config written while the daemon still kept `preferred_provider` loads,
/// and the next write leaves the key out. The key is ignored rather than
/// refused, like any field the daemon no longer has.
#[test]
fn a_stored_preferred_provider_is_dropped() {
    let config = from_ron(
        r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    transcription: (
        preferred_model: "voxtral-mini",
        preferred_provider: "local_voxtral",
        preferred_source: "github.com/super-stt/voxtral",
    ),
)"#,
    );
    assert_eq!(config.transcription.preferred_model, "voxtral-mini");
    assert_eq!(
        config.transcription.preferred_source,
        "github.com/super-stt/voxtral"
    );
    let stored = written(&config);
    assert!(!stored.contains("preferred_provider"), "{stored}");
}

/// A stored method round-trips.
#[test]
fn daemon_config_reads_notification_method() {
    let cfg = from_ron(
        r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    notification_method: "desktop",
    transcription: (preferred_model: "whisper"),
)"#,
    );
    assert_eq!(cfg.notification_method, NotificationMethod::Desktop);
}

/// Config-load resilience: an unknown stored value degrades to the default
/// instead of failing the whole parse. (The wire setter, by contrast, rejects
/// it — see the endpoint doc.)
#[test]
fn daemon_bad_notification_method_falls_back_preserving_rest() {
    let cfg = from_ron(
        r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    transcription: (
        preferred_model: "whisper",
        notification_method: "BogusMethod",
        write_method: "ydotool",
    ),
)"#,
    );
    assert_eq!(cfg.notification_method, NotificationMethod::default());
    assert_eq!(cfg.transcription.write_method, WriteMethod::Ydotool);
}

#[test]
fn default_update_config() {
    let config = DaemonConfig::default();
    assert!(config.update.check_enabled);
    assert_eq!(
        config.update.beta_optin,
        super_voice_shared::models::update_beta_optin::UpdateBetaOptIn::Auto
    );
}

#[test]
fn bad_beta_optin_falls_back_preserving_rest() {
    // Mirror the shape of `daemon_bad_theme_falls_back_preserving_rest`: one
    // bad enum value must reset only that field, not wipe the config.
    let cfg = from_ron(
        r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 80),
    transcription: (preferred_model: "whisper-tiny"),
    update: (check_enabled: false, beta_optin: "yes-please"),
)"#,
    );
    assert_eq!(
        cfg.update.beta_optin,
        super_voice_shared::models::update_beta_optin::UpdateBetaOptIn::Auto
    );
    assert!(!cfg.update.check_enabled); // sibling field preserved
    assert_eq!(cfg.audio.volume, 80); // other sections preserved
}

#[test]
fn update_config_round_trips() {
    let mut cfg = DaemonConfig::default();
    cfg.update.check_enabled = false;
    cfg.update.beta_optin = super_voice_shared::models::update_beta_optin::UpdateBetaOptIn::Enabled;
    let serialized = written(&cfg);
    assert!(serialized.contains("beta_optin: \"enabled\""));
    let back = from_ron(&serialized);
    assert_eq!(back.update, cfg.update);
}

/// `rename_active_backend` (an install-time directory migration of the *same*
/// backend) must not disturb the loaded-model preference the way
/// `update_active_backend` (a user picking a *different* backend) does —
/// otherwise updating a backend silently forgets the user's selected model.
#[test]
fn rename_active_backend_preserves_the_model_preference() {
    let mut c = DaemonConfig::default();
    c.transcription.active_backend = Some("super-stt-voxtral".to_string());
    c.update_preferred_model(
        "voxtral-mini".to_string(),
        "github.com/super-stt/voxtral".to_string(),
    );

    c.rename_active_backend("app.super-stt.voxtral".to_string());

    assert_eq!(
        c.transcription.active_backend.as_deref(),
        Some("app.super-stt.voxtral")
    );
    assert_eq!(c.transcription.preferred_model, "voxtral-mini");
}

/// The serde defaults and `Default::default()` must agree on every field. They
/// are written in two places, and a config round-tripping through
/// `DaemonConfig::default()` would otherwise behave differently from one read
/// from a file with an empty section.
#[test]
fn tcp_defaults_are_the_same_whether_constructed_or_deserialized() {
    let from_empty_section = from_ron(
        r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    transcription: (),
    http: (tcp: ()),
)"#,
    );
    assert_eq!(
        from_empty_section.http.tcp,
        DaemonConfig::default().http.tcp
    );
    assert_ne!(
        from_empty_section.http.tcp.port, 0,
        "0 would mean OS-chosen"
    );
}

/// An explicitly empty list is the lockdown, and has to stay distinct from the
/// permissive default — otherwise "I listed no origins" would read as "I listed
/// them all", which is the wrong way round for a field to fail.
#[test]
fn an_enabled_listener_with_no_origins_allows_nothing() {
    let tcp = crate::config::TcpConfig {
        enabled: true,
        port: crate::config::DEFAULT_TCP_PORT,
        allowed_origins: Vec::new(),
    };
    assert!(tcp.bind_addr().is_some(), "the listener is on");
    assert!(!tcp.admits_any_origin());
    assert!(!tcp.is_origin_allowed("http://127.0.0.1:8910"));
    assert!(!tcp.is_origin_allowed(""));
}

/// The default admits any origin. Consent, not this list, is what a page has to
/// get past — so the wildcard is a real setting rather than a way of skipping
/// the check.
#[test]
fn the_default_origin_list_admits_anything() {
    let tcp = crate::config::TcpConfig::default();
    assert!(tcp.admits_any_origin());
    for origin in [
        "http://localhost:8910",
        "https://example.test",
        "http://127.0.0.1:1",
    ] {
        assert!(tcp.is_origin_allowed(origin), "{origin} should be admitted");
    }
}

/// A wildcard alongside named origins still admits everything. The list is read
/// as a set of permissions, not as an ordered ruleset with overrides, so a
/// stray `*` cannot be narrowed by the entries beside it.
#[test]
fn a_wildcard_beside_named_origins_still_admits_anything() {
    let tcp = crate::config::TcpConfig {
        enabled: true,
        port: crate::config::DEFAULT_TCP_PORT,
        allowed_origins: vec![
            "http://localhost:8910".to_string(),
            crate::config::ANY_ORIGIN.to_string(),
        ],
    };
    assert!(tcp.is_origin_allowed("https://anything.test"));
}

/// Origins match exactly. A prefix or suffix test here would admit
/// `http://127.0.0.1:8910.evil.test`, which is a different site entirely.
#[test]
fn origin_matching_is_exact() {
    let tcp = crate::config::TcpConfig {
        enabled: true,
        port: crate::config::DEFAULT_TCP_PORT,
        allowed_origins: vec!["http://127.0.0.1:8910".to_string()],
    };
    assert!(tcp.is_origin_allowed("http://127.0.0.1:8910"));
    for near_miss in [
        "http://127.0.0.1:8910.evil.test",
        "http://127.0.0.1:8910/",
        "https://127.0.0.1:8910",
        "http://127.0.0.1:89100",
        "http://localhost:8910",
        "HTTP://127.0.0.1:8910",
    ] {
        assert!(
            !tcp.is_origin_allowed(near_miss),
            "{near_miss} is not the allowed origin"
        );
    }
}

/// The listener binds loopback whatever else is configured: nothing in the
/// consent flow is prepared for a caller on another host.
#[test]
fn the_tcp_listener_binds_loopback_only() {
    let tcp = crate::config::TcpConfig {
        enabled: true,
        port: crate::config::DEFAULT_TCP_PORT,
        allowed_origins: Vec::new(),
    };
    let addr = tcp.bind_addr().expect("enabled");
    assert!(addr.ip().is_loopback());
    assert_eq!(addr.port(), crate::config::DEFAULT_TCP_PORT);
}

/// The default port stays inside the family's reserved block, and out of the
/// ranges the system hands out on its own.
///
/// This is the one number that cannot be changed quietly: it is baked into
/// every user's stored config, every browser client's allowlist, and the
/// `servers` entry of the published protocol document. Asserting the
/// constraints here means a change to it has to be deliberate enough to also
/// change this test.
#[test]
fn the_default_port_sits_in_the_reserved_super_block() {
    const SUPER_BLOCK: std::ops::RangeInclusive<u16> = 7300..=7309;
    let port = crate::config::DEFAULT_TCP_PORT;

    assert!(
        SUPER_BLOCK.contains(&port),
        "{port} is outside the Super family block {SUPER_BLOCK:?}"
    );
    assert!(
        port >= 1024,
        "{port} is a privileged port; the daemon does not run as root"
    );
    assert!(
        !(30000..=32767).contains(&port),
        "{port} is in the Kubernetes NodePort range"
    );
    assert!(
        !(32768..=60999).contains(&port),
        "{port} is in Linux's default ephemeral range, where the kernel hands \
         the port to outgoing connections and the daemon loses the race at boot"
    );
}

/// A context named `id`, with a term and a prompt.
fn ctx(id: &str, name: &str) -> DictationContext {
    DictationContext {
        id: id.to_string(),
        name: name.to_string(),
        prompt: format!("I dictate {name}."),
        vocabulary: vec!["main branch".to_string(), "rebase".to_string()],
    }
}

/// Contexts survive the file, and survive it *in the user's order*.
///
/// The order is the reason this is a `Vec` and not a map: it is what the list
/// on the Contexts page shows, and a `HashMap` would reshuffle it on every
/// load — which `DaemonConfig::load` would then write back out, churning
/// the config file for no reason.
#[test]
fn contexts_round_trip_through_the_file_in_order() {
    let mut config = DaemonConfig::default();
    config.upsert_context(ctx("coding", "Coding"));
    config.upsert_context(ctx("email", "Email"));
    config.upsert_context(ctx("standup", "Standup notes"));
    config.set_active_context(Some("coding".to_string()));

    let parsed = round_trip(&config);

    let ids: Vec<&str> = parsed
        .contexts
        .items
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(ids, ["coding", "email", "standup"]);
    assert_eq!(
        parsed.active_context().map(|c| c.name.as_str()),
        Some("Coding")
    );
    assert_eq!(
        parsed.context("email").map(|c| c.vocabulary.len()),
        Some(2),
        "a context keeps its vocabulary across the file"
    );
}

/// An upsert replaces in place rather than appending a second copy, and a new
/// one lands at the end — where the user just added it.
#[test]
fn upserting_replaces_in_place_and_appends_new() {
    let mut config = DaemonConfig::default();
    config.upsert_context(ctx("coding", "Coding"));
    config.upsert_context(ctx("email", "Email"));

    let mut renamed = ctx("coding", "Coding");
    renamed.name = "Programming".to_string();
    config.upsert_context(renamed);

    assert_eq!(config.contexts.items.len(), 2, "no duplicate was appended");
    assert_eq!(
        config.context("coding").map(|c| c.name.as_str()),
        Some("Programming")
    );
    assert_eq!(config.contexts.items[0].id, "coding", "order is unchanged");
}

/// Deleting the active context clears the selection, so nothing resolves to a
/// context that is gone.
#[test]
fn deleting_the_active_context_clears_the_selection() {
    let mut config = DaemonConfig::default();
    config.upsert_context(ctx("coding", "Coding"));
    config.upsert_context(ctx("email", "Email"));
    config.set_active_context(Some("coding".to_string()));

    assert!(config.remove_context("coding"));
    assert_eq!(config.contexts.active, None);
    assert!(config.active_context().is_none());

    // Deleting one that is not active leaves the selection alone.
    config.set_active_context(Some("email".to_string()));
    assert!(!config.remove_context("coding"), "already gone");
    assert_eq!(
        config.active_context().map(|c| c.id.as_str()),
        Some("email")
    );
}

/// A selection naming a context that no longer exists reads as nothing.
///
/// The id is kept in the file rather than pruned on load, so restoring the
/// context restores the selection; what must not happen is resolving it.
#[test]
fn an_active_id_with_no_context_resolves_to_nothing() {
    let mut config = DaemonConfig::default();
    config.set_active_context(Some("ghost".to_string()));
    assert!(config.active_context().is_none());
    assert_eq!(
        config.contexts.active.as_deref(),
        Some("ghost"),
        "kept, not pruned"
    );
}

/// The three states a backend can be in, which is the whole point of the
/// override being a map with a sentinel rather than a bool.
#[test]
fn a_backend_follows_the_active_context_unless_it_is_pinned() {
    let mut config = DaemonConfig::default();
    config.upsert_context(ctx("coding", "Coding"));
    config.upsert_context(ctx("email", "Email"));
    config.set_active_context(Some("coding".to_string()));
    let openai = "github.com/super-stt/openai";

    // Unpinned: follows the active context.
    assert_eq!(
        config.resolve_context(openai).map(|c| c.id.as_str()),
        Some("coding")
    );

    // Pinned elsewhere: takes what it was pinned to, active or not.
    config.update_backend_context(openai.to_string(), Some("email".to_string()));
    assert_eq!(
        config.resolve_context(openai).map(|c| c.id.as_str()),
        Some("email")
    );

    // Pinned to nothing: gets nothing, even though a context is active.
    config.update_backend_context(openai.to_string(), Some(String::new()));
    assert!(config.resolve_context(openai).is_none());
    assert!(
        config.active_context().is_some(),
        "the active one is untouched"
    );

    // Unpinned again: back to following.
    config.update_backend_context(openai.to_string(), None);
    assert_eq!(
        config.resolve_context(openai).map(|c| c.id.as_str()),
        Some("coding")
    );
    assert!(
        config.backends.context_override.is_empty(),
        "clearing prunes the entry"
    );
}

/// A pin to a context that has since been deleted sends nothing, rather than
/// quietly falling back to the active one.
///
/// "Use this context" and "use whatever is active" are different instructions.
/// Promoting the first to the second would send, say, coding vocabulary to a
/// backend the user had deliberately pointed somewhere else.
#[test]
fn a_pin_to_a_deleted_context_does_not_fall_back_to_the_active_one() {
    let mut config = DaemonConfig::default();
    config.upsert_context(ctx("coding", "Coding"));
    config.upsert_context(ctx("medical", "Medical notes"));
    config.set_active_context(Some("coding".to_string()));
    let openai = "github.com/super-stt/openai";
    config.update_backend_context(openai.to_string(), Some("medical".to_string()));

    config.remove_context("medical");

    assert!(config.resolve_context(openai).is_none());
    assert_eq!(
        config.backend_context_override(openai),
        Some("medical"),
        "the pin is kept, so restoring the context restores it"
    );
}

/// The listener's default port is the one `SUPER_VOICE` names. The two are
/// written apart, a const parameter and a static, so this is what keeps them
/// the same number.
#[test]
fn the_default_tcp_port_is_super_stts() {
    assert_eq!(
        crate::config::DEFAULT_TCP_PORT,
        super_voice_shared::product::SUPER_VOICE.tcp_port
    );
}
