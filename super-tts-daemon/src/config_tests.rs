// SPDX-License-Identifier: GPL-3.0-only
use super::*;

/// A config as the daemon stores it. Fails the test if it would have been
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

#[test]
fn default_config_has_online_models_disabled() {
    let config = DaemonConfig::default();
    assert!(!config.online.allow_online_models);
}

#[test]
fn config_without_online_section_deserializes() {
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    synthesis: (preferred_model: "kokoro-tiny"),
)"#;
    let config = from_ron(ron_str);
    assert!(!config.online.allow_online_models);
}

#[test]
fn config_with_online_section_round_trips() {
    let mut config = DaemonConfig::default();
    config.online.allow_online_models = true;

    let parsed = round_trip(&config);
    assert!(parsed.online.allow_online_models);
}

#[test]
fn config_with_online_model_preferred_round_trips() {
    let mut config = DaemonConfig::default();
    config.online.allow_online_models = true;
    config.synthesis.preferred_model = "kokoro-1".to_string();

    let parsed = round_trip(&config);
    assert!(parsed.online.allow_online_models);
    assert_eq!(parsed.synthesis.preferred_model, "kokoro-1");
}

#[test]
fn config_preserves_all_online_model_variants() {
    for name in [
        "kokoro-1",
        "gpt-4o-mini-tts",
        "tts-1-hd",
        "eleven-multilingual-v2",
        "nova-3",
    ] {
        let model = name.to_string();
        let mut config = DaemonConfig::default();
        config.synthesis.preferred_model = model.clone();

        let parsed = round_trip(&config);
        assert_eq!(parsed.synthesis.preferred_model, model);
    }
}

#[test]
fn online_config_default_is_disabled() {
    let online = OnlineConfig::default();
    assert!(!online.allow_online_models);
}

#[test]
fn default_config_has_no_custom_models_dir() {
    let config = DaemonConfig::default();
    assert!(config.synthesis.custom_models_dir.is_none());
}

#[test]
fn config_without_custom_models_dir_deserializes() {
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    synthesis: (preferred_model: "kokoro-tiny"),
)"#;
    let config = from_ron(ron_str);
    assert!(config.synthesis.custom_models_dir.is_none());
}

#[test]
fn config_with_custom_models_dir_round_trips() {
    let mut config = DaemonConfig::default();
    config.synthesis.custom_models_dir = Some("/tmp/models".to_string());

    let parsed = round_trip(&config);
    assert_eq!(
        parsed.synthesis.custom_models_dir.as_deref(),
        Some("/tmp/models")
    );
}

#[test]
fn config_with_none_custom_models_dir_round_trips() {
    let config = DaemonConfig::default();

    let parsed = round_trip(&config);
    assert!(parsed.synthesis.custom_models_dir.is_none());
}

#[test]
fn backend_options_set_clear_and_round_trip() {
    let mut config = DaemonConfig::default();
    let src = "github.com/super-tts/openai";

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

#[test]
fn config_without_backends_section_deserializes() {
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    synthesis: (preferred_model: "kokoro-1"),
)"#;
    let config = from_ron(ron_str);
    assert!(config.backends.options.is_empty());
}

/// A config carrying a stale provider/source string
/// (e.g. `PascalCase` variant names from a prior build) must keep loading
/// — falling back to the type's `Default` rather than failing the whole
/// `[synthesis]` section. The user's other settings have to survive.
#[test]
fn config_with_legacy_provider_string_falls_back() {
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "silent", volume: 75),
    synthesis: (
        preferred_model: "kokoro-tiny",
        preferred_provider: "LocalKokoro",
        preferred_source: "BadValue",
    ),
    online: (allow_online_models: true),
)"#;
    let config = from_ron(ron_str);
    // The source is a free-form string, so any value is accepted. The legacy
    // provider key is ignored, and the next write leaves it out.
    assert_eq!(config.synthesis.preferred_source, "BadValue");
    assert!(!written(&config).contains("preferred_provider"));
    // Other fields must survive the field-level fallback.
    assert_eq!(config.synthesis.preferred_model, "kokoro-tiny");
    assert_eq!(config.audio.theme, AudioTheme::Silent);
    assert_eq!(config.audio.volume, 75);
    assert!(config.online.allow_online_models);
}

/// `synthesis.active_backend` defaults to `None` (no backend selected
/// at install time → daemon idle).
#[test]
fn active_backend_default_is_none() {
    let config = DaemonConfig::default();
    assert!(config.synthesis.active_backend.is_none());
}

/// A persisted relative dir round-trips through the file, preserving the
/// stable handle the daemon uses to find the backend on restart.
#[test]
fn active_backend_round_trips_through_the_file() {
    let mut config = DaemonConfig::default();
    config.synthesis.active_backend = Some("mistral".to_string());

    let serialized = written(&config);
    assert!(
        serialized.contains("active_backend: Some(\"mistral\")"),
        "serialized form: {serialized}"
    );

    let parsed = from_ron(&serialized);
    assert_eq!(parsed.synthesis.active_backend.as_deref(), Some("mistral"));
}

/// A pre-existing config that predates the `active_backend` field must
/// still deserialize cleanly (the field is `Option`-with-default).
#[test]
fn config_without_active_backend_field_deserializes() {
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    synthesis: (preferred_model: "kokoro-tiny"),
)"#;
    let config = from_ron(ron_str);
    assert!(config.synthesis.active_backend.is_none());
}

#[test]
fn daemon_bad_theme_falls_back_preserving_rest() {
    // A single unrecognized enum value must NOT wipe the whole config.
    let ron_str = r#"(
    device: (preferred_device: "gpu"),
    audio: (theme: "Nonexistent", volume: 80),
    synthesis: (preferred_model: "KokoroTiny"),
)"#;
    let cfg = from_ron(ron_str);
    assert_eq!(cfg.audio.theme, AudioTheme::default()); // bad field reset
    assert_eq!(cfg.audio.volume, 80); // everything else preserved
    assert_eq!(cfg.device.preferred_device, "gpu");
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
             synthesis: (preferred_model: \"\"))"
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

#[test]
fn cleared_preferred_model_persists_as_idle_with_backend_kept() {
    // Invariant behind the unload path (`clear_preferred_model`): dropping the
    // loaded model empties preferred_model/source but keeps the active backend
    // selected, and that state must survive a save/reload so a daemon restart
    // stays idle instead of reloading the just-unloaded model.
    let mut config = DaemonConfig::default();
    config.synthesis.preferred_model = "kokoro-large-v3".to_string();
    config.synthesis.preferred_source = "openai-kokoro".to_string();
    config.synthesis.active_backend = Some("openai-kokoro".to_string());

    // Simulate the clear (the method itself also calls save(), which touches
    // the real config path, so exercise the field effect directly).
    config.synthesis.preferred_model = String::new();
    config.synthesis.preferred_source = String::new();

    let (parsed, was_reset) = DaemonConfig::parse_or_reset(&written(&config));
    assert!(!was_reset, "cleared config must re-parse cleanly");
    assert!(
        parsed.synthesis.preferred_model.is_empty(),
        "restart must not reload an unloaded model"
    );
    assert!(parsed.synthesis.preferred_source.is_empty());
    assert_eq!(
        parsed.synthesis.active_backend.as_deref(),
        Some("openai-kokoro"),
        "unload keeps the active backend selected"
    );
}

/// A stored method round-trips.
#[test]
fn daemon_config_reads_notification_method() {
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    synthesis: (preferred_model: "kokoro", notification_method: "off"),
)"#;
    let cfg = from_ron(ron_str);
    assert_eq!(cfg.synthesis.notification_method, NotificationMethod::Off);
}

/// Config-load resilience: an unknown stored value degrades to the default
/// instead of failing the whole parse. (The wire setter, by contrast, rejects
/// it — see the endpoint doc.)
#[test]
fn daemon_bad_notification_method_falls_back_preserving_rest() {
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    synthesis: (
        preferred_model: "kokoro",
        notification_method: "BogusMethod",
    ),
)"#;
    let cfg = from_ron(ron_str);
    assert_eq!(
        cfg.synthesis.notification_method,
        NotificationMethod::default()
    );
}

/// An absent field is the default.
#[test]
fn daemon_missing_notification_method_is_auto() {
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    synthesis: (preferred_model: "kokoro"),
)"#;
    let cfg = from_ron(ron_str);
    assert_eq!(cfg.synthesis.notification_method, NotificationMethod::Auto);
}

#[test]
fn default_update_config() {
    let config = DaemonConfig::default();
    assert!(config.update.check_enabled);
    assert_eq!(
        config.update.beta_optin,
        super_tts_shared::models::update_beta_optin::UpdateBetaOptIn::Auto
    );
}

#[test]
fn config_without_update_section_gets_defaults() {
    // Mirror the literal-TOML shape of `config_without_online_section_deserializes`
    // above: a full config WITHOUT an [update] section.
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    synthesis: (preferred_model: "kokoro-tiny"),
)"#;
    let cfg = from_ron(ron_str);
    assert!(cfg.update.check_enabled);
}

#[test]
fn bad_beta_optin_falls_back_preserving_rest() {
    // Mirror the shape of `daemon_bad_theme_falls_back_preserving_rest`: one
    // bad enum value must reset only that field, not wipe the config.
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 80),
    synthesis: (preferred_model: "kokoro-tiny"),
    update: (check_enabled: false, beta_optin: "yes-please"),
)"#;
    let cfg = from_ron(ron_str);
    assert_eq!(
        cfg.update.beta_optin,
        super_tts_shared::models::update_beta_optin::UpdateBetaOptIn::Auto
    );
    assert!(!cfg.update.check_enabled); // sibling field preserved
    assert_eq!(cfg.audio.volume, 80); // other sections preserved
}

#[test]
fn update_config_round_trips() {
    let mut cfg = DaemonConfig::default();
    cfg.update.check_enabled = false;
    cfg.update.beta_optin = super_tts_shared::models::update_beta_optin::UpdateBetaOptIn::Enabled;
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
    c.synthesis.active_backend = Some("super-tts-piper".to_string());
    c.update_preferred_model(
        "piper-mini".to_string(),
        "github.com/super-tts/piper".to_string(),
    );

    c.rename_active_backend("app.super-tts.piper".to_string());

    assert_eq!(
        c.synthesis.active_backend.as_deref(),
        Some("app.super-tts.piper")
    );
    assert_eq!(c.synthesis.preferred_model, "piper-mini");
}

/// The `http` section is optional: a config written before it existed
/// loads, and gets the same listener a fresh install gets. Upgrading a config
/// is not a way to end up with a daemon that behaves differently from a new one.
#[test]
fn config_without_http_section_gets_the_default_listener() {
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    synthesis: (preferred_model: "kokoro-tiny"),
)"#;
    let config = from_ron(ron_str);
    assert!(config.http.tcp.enabled);
    assert!(config.http.tcp.admits_any_origin());
    assert_eq!(
        config.http.tcp.bind_addr(),
        DaemonConfig::default().http.tcp.bind_addr(),
        "an upgraded config listens exactly where a fresh one does"
    );
}

/// The serde defaults and `Default::default()` must agree on every field. They
/// are written in two places, and a config round-tripping through
/// `DaemonConfig::default()` would otherwise behave differently from one read
/// from a file with an empty section.
#[test]
fn tcp_defaults_are_the_same_whether_constructed_or_deserialized() {
    let ron_str = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 100),
    synthesis: (preferred_model: ""),
    http: (tcp: ()),
)"#;
    let from_empty_section = from_ron(ron_str);
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

/// Super TTS takes `7301`, the second port in the family block; Super STT has
/// `7300`. The two daemons run side by side on the same desktop, so a shared
/// port would mean whichever started second lost its browser transport.
#[test]
fn super_tts_owns_the_second_port_in_the_block() {
    assert_eq!(crate::config::DEFAULT_TCP_PORT, 7301);
}

/// The listener's default port is the one `SUPER_TTS` names. The two are
/// written apart, a const parameter and a static, so this is what keeps them
/// the same number.
#[test]
fn the_default_tcp_port_is_super_ttss() {
    assert_eq!(
        crate::config::DEFAULT_TCP_PORT,
        super_tts_shared::product::SUPER_TTS.tcp_port
    );
}
