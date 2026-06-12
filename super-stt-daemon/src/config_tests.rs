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
theme = "Classic"
volume = 100

[transcription]
preferred_model = "whisper-tiny"
write_mode = false
preview_typing_enabled = false
recording_stop_mode = "SilenceAndManual"
write_method = "Auto"
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
theme = "Classic"
volume = 100

[transcription]
preferred_model = "whisper-tiny"
write_mode = false
preview_typing_enabled = false
recording_stop_mode = "SilenceAndManual"
write_method = "Auto"
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
theme = "Classic"
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
theme = "Silent"
volume = 75

[transcription]
preferred_model = "whisper-tiny"
preferred_provider = "LocalWhisper"
preferred_source = "BadValue"
write_mode = false
preview_typing_enabled = true
recording_stop_mode = "SilenceAndManual"
write_method = "Auto"

[online]
allow_online_models = true
"#;
    let config: DaemonConfig =
        toml::from_str(toml_str).expect("legacy provider string should not fail the whole config");
    assert_eq!(config.transcription.preferred_provider, Provider::default());
    // `preferred_source` is a free-form string now — any value is accepted.
    assert_eq!(config.transcription.preferred_source, "BadValue");
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
theme = "Classic"
volume = 100

[transcription]
preferred_model = "whisper-base"
preferred_provider = "local_voxtral"
preferred_source = "github.com/super-stt/voxtral"
write_mode = false
preview_typing_enabled = false
recording_stop_mode = "SilenceAndManual"
write_method = "Auto"
"#;
    let config: DaemonConfig = toml::from_str(toml_str).expect("should deserialize");
    assert_eq!(
        config.transcription.preferred_provider,
        Provider::from("local_voxtral")
    );
    assert_eq!(
        config.transcription.preferred_source,
        "github.com/super-stt/voxtral"
    );

    let serialized = toml::to_string_pretty(&config).expect("should serialize");
    assert!(
        serialized.contains("preferred_provider = \"local_voxtral\""),
        "serialized form: {serialized}"
    );
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
theme = "Classic"
volume = 100

[transcription]
preferred_model = "whisper-tiny"
write_mode = false
"#;
    let config: DaemonConfig = toml::from_str(toml_str).expect("should deserialize");
    assert!(config.transcription.active_backend.is_none());
}
