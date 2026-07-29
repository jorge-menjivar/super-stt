// SPDX-License-Identifier: GPL-3.0-only
use super::*;

#[test]
fn default_config_has_online_models_disabled() {
    let config = DaemonConfig::default();
    assert!(!config.online.allow_online_models);
}

#[test]
fn config_without_online_section_deserializes() {
    let toml_str = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "classic"
volume = 100

[transcription]
preferred_model = "whisper-tiny"
write_mode = false
preview_typing_enabled = false
recording_stop_mode = "silence_and_manual"
write_method = "auto"
"#;
    let config: DaemonConfig = toml::from_str(toml_str).expect("should deserialize");
    assert!(!config.online.allow_online_models);
}

#[test]
fn config_with_online_section_round_trips() {
    let mut config = DaemonConfig::default();
    config.online.allow_online_models = true;

    let toml_str = toml::to_string_pretty(&config).expect("should serialize");
    let parsed: DaemonConfig = toml::from_str(&toml_str).expect("should deserialize");
    assert!(parsed.online.allow_online_models);
}

#[test]
fn config_with_online_model_preferred_round_trips() {
    let mut config = DaemonConfig::default();
    config.online.allow_online_models = true;
    config.transcription.preferred_model = "whisper-1".to_string();

    let toml_str = toml::to_string_pretty(&config).expect("should serialize");
    let parsed: DaemonConfig = toml::from_str(&toml_str).expect("should deserialize");
    assert!(parsed.online.allow_online_models);
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

        let toml_str = toml::to_string_pretty(&config).expect("should serialize");
        let parsed: DaemonConfig = toml::from_str(&toml_str).expect("should deserialize");
        assert_eq!(parsed.transcription.preferred_model, model);
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
    assert!(config.transcription.custom_models_dir.is_none());
}

#[test]
fn config_without_custom_models_dir_deserializes() {
    let toml_str = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "classic"
volume = 100

[transcription]
preferred_model = "whisper-tiny"
write_mode = false
preview_typing_enabled = false
recording_stop_mode = "silence_and_manual"
write_method = "auto"
"#;
    let config: DaemonConfig = toml::from_str(toml_str).expect("should deserialize");
    assert!(config.transcription.custom_models_dir.is_none());
}

#[test]
fn config_with_custom_models_dir_round_trips() {
    let mut config = DaemonConfig::default();
    config.transcription.custom_models_dir = Some("/tmp/models".to_string());

    let toml_str = toml::to_string_pretty(&config).expect("should serialize");
    let parsed: DaemonConfig = toml::from_str(&toml_str).expect("should deserialize");
    assert_eq!(
        parsed.transcription.custom_models_dir.as_deref(),
        Some("/tmp/models")
    );
}

#[test]
fn config_with_none_custom_models_dir_round_trips() {
    let config = DaemonConfig::default();

    let toml_str = toml::to_string_pretty(&config).expect("should serialize");
    let parsed: DaemonConfig = toml::from_str(&toml_str).expect("should deserialize");
    assert!(parsed.transcription.custom_models_dir.is_none());
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

    // Survives a TOML round-trip.
    let toml_str = toml::to_string_pretty(&config).expect("serialize");
    let parsed: DaemonConfig = toml::from_str(&toml_str).expect("deserialize");
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
    let toml_str = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "classic"
volume = 100

[transcription]
preferred_model = "whisper-1"
write_mode = false
"#;
    let config: DaemonConfig = toml::from_str(toml_str).expect("should deserialize");
    assert!(config.backends.options.is_empty());
}

/// A pre-existing TOML config carrying a stale provider/source string
/// (e.g. PascalCase variant names from a prior build) must keep loading
/// — falling back to the type's `Default` rather than failing the whole
/// `[transcription]` section. The user's other settings have to survive.
#[test]
fn config_with_legacy_provider_string_falls_back() {
    let toml_str = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "silent"
volume = 75

[transcription]
preferred_model = "whisper-tiny"
preferred_provider = "LocalWhisper"
preferred_source = "BadValue"
write_mode = false
preview_typing_enabled = true
recording_stop_mode = "silence_and_manual"
write_method = "auto"

[online]
allow_online_models = true
"#;
    let config: DaemonConfig =
        toml::from_str(toml_str).expect("legacy provider string should not fail the whole config");
    // Both are free-form strings now — any value is accepted, and the legacy
    // provider is carried through rather than rejected or dropped.
    assert_eq!(config.transcription.preferred_source, "BadValue");
    assert_eq!(config.transcription.preferred_provider, "LocalWhisper");
    // Other fields must survive the field-level fallback.
    assert_eq!(config.transcription.preferred_model, "whisper-tiny");
    assert!(config.transcription.preview_typing_enabled);
    assert_eq!(config.audio.theme, AudioTheme::Silent);
    assert_eq!(config.audio.volume, 75);
    assert!(config.online.allow_online_models);
}

#[test]
fn config_with_canonical_snake_case_provider_round_trips() {
    let toml_str = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "classic"
volume = 100

[transcription]
preferred_model = "whisper-base"
preferred_provider = "local_voxtral"
preferred_source = "github.com/super-stt/voxtral"
write_mode = false
preview_typing_enabled = false
recording_stop_mode = "silence_and_manual"
write_method = "auto"
"#;
    let config: DaemonConfig = toml::from_str(toml_str).expect("should deserialize");
    assert_eq!(
        config.transcription.preferred_source,
        "github.com/super-stt/voxtral"
    );
    assert_eq!(config.transcription.preferred_provider, "local_voxtral");
}

/// `transcription.active_backend` defaults to `None` (no backend selected
/// at install time → daemon idle).
#[test]
fn active_backend_default_is_none() {
    let config = DaemonConfig::default();
    assert!(config.transcription.active_backend.is_none());
}

/// A persisted relative dir round-trips through TOML, preserving the
/// stable handle the daemon uses to find the backend on restart.
#[test]
fn active_backend_round_trips_through_toml() {
    let mut config = DaemonConfig::default();
    config.transcription.active_backend = Some("mistral".to_string());

    let serialized = toml::to_string_pretty(&config).expect("serialize");
    assert!(
        serialized.contains("active_backend = \"mistral\""),
        "serialized form: {serialized}"
    );

    let parsed: DaemonConfig = toml::from_str(&serialized).expect("deserialize");
    assert_eq!(
        parsed.transcription.active_backend.as_deref(),
        Some("mistral")
    );
}

/// A pre-existing config that predates the `active_backend` field must
/// still deserialize cleanly (the field is `Option`-with-default).
#[test]
fn config_without_active_backend_field_deserializes() {
    let toml_str = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "classic"
volume = 100

[transcription]
preferred_model = "whisper-tiny"
write_mode = false
"#;
    let config: DaemonConfig = toml::from_str(toml_str).expect("should deserialize");
    assert!(config.transcription.active_backend.is_none());
}

#[test]
fn daemon_bad_theme_falls_back_preserving_rest() {
    // A single unrecognized enum value must NOT wipe the whole config.
    let toml_str = r#"
[device]
preferred_device = "cuda"

[audio]
theme = "Nonexistent"
volume = 80

[transcription]
preferred_model = "WhisperTiny"
write_mode = true
recording_stop_mode = "manual_only"
write_method = "ydotool"
"#;
    let cfg: DaemonConfig = toml::from_str(toml_str).expect("must parse, not error");
    assert_eq!(cfg.audio.theme, AudioTheme::default()); // bad field reset
    assert_eq!(cfg.audio.volume, 80); // everything else preserved
    assert_eq!(cfg.device.preferred_device, "cuda");
    assert!(cfg.transcription.write_mode);
    assert_eq!(
        cfg.transcription.recording_stop_mode,
        RecordingStopMode::ManualOnly
    );
    assert_eq!(cfg.transcription.write_method, WriteMethod::Ydotool);
}

#[test]
fn daemon_bad_stop_mode_and_write_method_fall_back_preserving_rest() {
    let toml_str = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "gentle"
volume = 100

[transcription]
preferred_model = "WhisperTiny"
write_mode = false
recording_stop_mode = "BogusMode"
write_method = "BogusMethod"
"#;
    let cfg: DaemonConfig = toml::from_str(toml_str).expect("must parse, not error");
    assert_eq!(cfg.audio.theme, AudioTheme::Gentle); // preserved
    assert_eq!(
        cfg.transcription.recording_stop_mode,
        RecordingStopMode::default()
    );
    assert_eq!(cfg.transcription.write_method, WriteMethod::default());
}

#[test]
fn corrupt_daemon_config_resets_to_default() {
    let (cfg, was_reset) = DaemonConfig::parse_or_reset("this is not ::: valid toml [");
    assert!(was_reset, "garbage input must trigger a reset");
    // The reset config is a valid default (proves no panic, app can start).
    assert_eq!(cfg.audio.theme, AudioTheme::default());
    assert_eq!(cfg.device.preferred_device, "cpu");
}

/// The committed v0.1.3 `daemon.toml` fixture (customized, not defaults). The
/// canonical copy lives in the on-disk corpus so the release gate and these
/// detailed assertions test the same bytes.
fn v0_1_3_daemon_fixture() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/configs/v0.1.3/daemon.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn v0_1_3_full_daemon_config_loads_and_migrates() {
    let (cfg, was_reset) = DaemonConfig::parse_or_reset(&v0_1_3_daemon_fixture());
    assert!(!was_reset, "a valid v0.1.3 config must load, not reset");

    // Preserved fields.
    assert_eq!(cfg.device.preferred_device, "cuda");
    assert_eq!(cfg.audio.volume, 80);
    assert!(cfg.transcription.write_mode);
    assert!(!cfg.transcription.preview_typing_enabled);
    assert!(cfg.online.allow_online_models);

    // Settings-enum fields migrate to default: v0.1.3 persisted them in the old
    // PascalCase form (`Gentle`/`ManualOnly`/`Ydotool`), which the snake_case
    // wire/config form no longer recognizes, so `deserialize_or_default` degrades
    // each to its default rather than failing the whole load. (The whole config
    // still loads cleanly — `was_reset` is false above.)
    assert_eq!(cfg.audio.theme, AudioTheme::default());
    assert_eq!(
        cfg.transcription.recording_stop_mode,
        RecordingStopMode::default()
    );
    assert_eq!(cfg.transcription.write_method, WriteMethod::default());

    // `preferred_model` widened from STTModel enum to String: the old value is
    // retained verbatim (daemon model loader has its own fallback downstream).
    assert_eq!(cfg.transcription.preferred_model, "WhisperLargeV3Turbo");

    // Removed field dropped (no `deny_unknown_fields`); the replacement is None.
    assert_eq!(cfg.transcription.custom_models_dir, None);

    // New fields materialize at their defaults.
    assert_eq!(cfg.transcription.preferred_source, "");
    assert_eq!(cfg.transcription.backends_dir, None);
    assert_eq!(cfg.transcription.active_backend, None);
    assert_eq!(cfg.transcription.primary_language, None);
    assert!(cfg.backends.options.is_empty());
    assert!(cfg.backends.models.is_empty());
}

#[test]
fn v0_1_3_every_preferred_model_variant_loads() {
    // Every STTModel serde name v0.1.3 could have written to `preferred_model`.
    const V0_1_3_MODELS: &[&str] = &[
        "WhisperTiny",
        "WhisperTinyEn",
        "WhisperBase",
        "WhisperBaseEn",
        "WhisperSmall",
        "WhisperSmallEn",
        "WhisperMedium",
        "WhisperMediumEn",
        "WhisperLarge",
        "WhisperLargeV2",
        "WhisperLargeV3",
        "WhisperLargeV3Turbo",
        "WhisperDistilMediumEn",
        "WhisperDistilLargeV2",
        "WhisperDistilLargeV3",
        "VoxtralSmall",
        "VoxtralMini",
        "OpenAIWhisper1",
        "OpenAIGpt4oTranscribe",
        "OpenAIGpt4oMiniTranscribe",
        "MistralVoxtralMiniTranscribeV2",
        "DeepgramNova3",
    ];
    for model in V0_1_3_MODELS {
        let toml_str = format!(
            "[device]\npreferred_device = \"cpu\"\n\
             [audio]\ntheme = \"Classic\"\nvolume = 100\n\
             [transcription]\npreferred_model = \"{model}\"\nwrite_mode = false\n"
        );
        let (cfg, was_reset) = DaemonConfig::parse_or_reset(&toml_str);
        assert!(!was_reset, "v0.1.3 model {model} must load, not reset");
        assert_eq!(cfg.transcription.preferred_model, *model);
    }
}

#[test]
fn v0_1_3_config_reserializes_to_stable_canonical() {
    // load() rewrites a migrated config in canonical form; that rewrite must
    // itself be a valid, stable current config (backends empty → no HashMap
    // ordering nondeterminism).
    let (cfg, _) = DaemonConfig::parse_or_reset(&v0_1_3_daemon_fixture());
    let s1 = toml::to_string_pretty(&cfg).expect("serialize migrated config");
    let (cfg2, was_reset) = DaemonConfig::parse_or_reset(&s1);
    assert!(!was_reset, "canonical rewrite must re-parse cleanly");
    let s2 = toml::to_string_pretty(&cfg2).expect("serialize round-trip");
    assert_eq!(s1, s2, "canonical form must be idempotent");
}

#[test]
fn cleared_preferred_model_persists_as_idle_with_backend_kept() {
    // Invariant behind the unload path (`clear_preferred_model`): dropping the
    // loaded model empties preferred_model/source but keeps the active backend
    // selected, and that state must survive a save/reload so a daemon restart
    // stays idle instead of reloading the just-unloaded model.
    let mut config = DaemonConfig::default();
    config.transcription.preferred_model = "whisper-large-v3".to_string();
    config.transcription.preferred_source = "openai-whisper".to_string();
    config.transcription.active_backend = Some("openai-whisper".to_string());

    // Simulate the clear (the method itself also calls save(), which touches
    // the real config path, so exercise the field effect directly).
    config.transcription.preferred_model = String::new();
    config.transcription.preferred_source = String::new();

    let toml_str = toml::to_string_pretty(&config).expect("should serialize");
    let (parsed, was_reset) = DaemonConfig::parse_or_reset(&toml_str);
    assert!(!was_reset, "cleared config must re-parse cleanly");
    assert!(
        parsed.transcription.preferred_model.is_empty(),
        "restart must not reload an unloaded model"
    );
    assert!(parsed.transcription.preferred_source.is_empty());
    assert_eq!(
        parsed.transcription.active_backend.as_deref(),
        Some("openai-whisper"),
        "unload keeps the active backend selected"
    );
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
        let fixture = version_dir.join("daemon.toml");
        if !fixture.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&fixture).expect("read daemon.toml fixture");
        let (_, was_reset) = DaemonConfig::parse_or_reset(&content);
        assert!(
            !was_reset,
            "daemon fixture {} must load cleanly (no reset)",
            fixture.display()
        );
        checked += 1;
    }
    assert!(
        checked >= 4,
        "expected >= 4 daemon fixtures (v0.1.0-v0.1.3), found {checked}"
    );
}

/// The regression: `preferred_provider` used to be dropped from the struct, so
/// the first save after an upgrade rewrote `daemon.toml` without it. Daemons
/// through v0.2.0 resolve their startup model by `(model, provider, source)`,
/// so a user rolling back after a bad upgrade got an idle daemon with their
/// selection silently gone — and `daemon.toml` outlives the binary that wrote
/// it, so nothing later can recover the value.
#[test]
fn a_save_preserves_preferred_provider() {
    let toml_str = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "classic"
volume = 100

[transcription]
preferred_model = "voxtral-mini"
preferred_provider = "local_voxtral"
preferred_source = "github.com/super-stt/voxtral"
write_mode = false
preview_typing_enabled = false
recording_stop_mode = "silence_and_manual"

[online]
allow_online_models = false
"#;
    let config: DaemonConfig = toml::from_str(toml_str).expect("fixture parses");
    assert_eq!(config.transcription.preferred_provider, "local_voxtral");

    let written = toml::to_string_pretty(&config).expect("serializes");
    assert!(
        written.contains("preferred_provider = \"local_voxtral\""),
        "a save dropped `preferred_provider`; a rollback to v0.2.0 comes up idle:\n{written}"
    );
}

/// Preserving the key is not enough on its own: a model switch under the new
/// daemon must carry the newly-selected model's provider into it. A value left
/// pointing at the *previous* model is exactly as unusable to a rolled-back
/// v0.2.0 daemon as a missing one — it resolves nothing and the daemon idles.
#[test]
fn a_model_switch_updates_preferred_provider() {
    let mut config = DaemonConfig::default();
    config.update_preferred_model(
        "voxtral-mini".to_string(),
        "github.com/super-stt/voxtral".to_string(),
        Some("local_voxtral".to_string()),
    );
    assert_eq!(config.transcription.preferred_provider, "local_voxtral");

    // Switching to a model from another backend must not leave the old one.
    config.update_preferred_model(
        "whisper-tiny".to_string(),
        "github.com/super-stt/whisper".to_string(),
        Some("local_whisper".to_string()),
    );
    assert_eq!(config.transcription.preferred_provider, "local_whisper");

    // A model whose manifest declares none clears it rather than keeping a
    // provider that belongs to a different model.
    config.update_preferred_model(
        "nova-3".to_string(),
        "github.com/super-stt/deepgram".to_string(),
        None,
    );
    assert_eq!(config.transcription.preferred_provider, "");
}

/// Every path that drops the model preference drops the provider with it —
/// otherwise the persisted triple names a model that is no longer selected.
#[test]
fn clearing_the_model_preference_clears_the_provider() {
    let seeded = || {
        let mut c = DaemonConfig::default();
        c.update_preferred_model(
            "voxtral-mini".to_string(),
            "github.com/super-stt/voxtral".to_string(),
            Some("local_voxtral".to_string()),
        );
        c
    };

    let mut c = seeded();
    c.clear_preferred_model();
    assert_eq!(c.transcription.preferred_provider, "");

    let mut c = seeded();
    c.update_active_backend("whisper".to_string());
    assert_eq!(
        c.transcription.preferred_provider, "",
        "selecting a backend drops the model preference; the provider must go too"
    );

    let mut c = seeded();
    c.clear_active_backend();
    assert_eq!(c.transcription.preferred_provider, "");
}
