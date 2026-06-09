// SPDX-License-Identifier: GPL-3.0-only
use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;

/// The required secrets / options that the user has not yet set for this
/// backend, by human-readable label. Returned in declaration order so the
/// inline warnings are stable across renders. Drives both the inline
/// "{label} must be set." rows and whether the Select button is enabled.
///
/// Takes the keyring "is this secret configured?" map directly rather than the
/// whole [`AppModel`] so the rule stays pure and unit-testable.
pub(super) fn unmet_requirements<'a>(
    secret_configured: &std::collections::HashMap<(String, String), bool>,
    backend: &'a BackendInfo,
) -> Vec<&'a str> {
    let mut out = Vec::new();
    for secret in &backend.secrets {
        if !secret.required {
            continue;
        }
        let configured = secret_configured
            .get(&(backend.source.clone(), secret.name.clone()))
            .copied()
            .unwrap_or(false);
        if !configured {
            out.push(secret.label.as_deref().unwrap_or(&secret.name));
        }
    }
    for option in &backend.options {
        if !option.required {
            continue;
        }
        let value = option.value.as_deref().unwrap_or("").trim();
        if value.is_empty() {
            out.push(option.label.as_deref().unwrap_or(&option.name));
        }
    }
    out
}

#[cfg(test)]
mod unmet_requirements_tests {
    //! Pin the rule for which secrets/options gate the per-backend Select
    //! button: only `required` ones, the keyring map decides per-secret, and
    //! the human-readable `label` (not the wire `name`) is what surfaces.
    use super::*;
    use crate::daemon::backends::{BackendInfo, BackendModel, BackendOption, BackendSecret};
    use std::collections::HashMap;

    fn backend(secrets: Vec<BackendSecret>, options: Vec<BackendOption>) -> BackendInfo {
        BackendInfo {
            source: "github.com/super-stt/openai".to_string(),
            name: "OpenAI".to_string(),
            models: vec![BackendModel {
                name: "whisper-1".to_string(),
                provider: "openai".to_string(),
                supported_devices: vec!["none".to_string()],
                estimated_vram_bytes: 0,
            }],
            secrets,
            options,
        }
    }

    fn secret(name: &str, label: Option<&str>, required: bool) -> BackendSecret {
        BackendSecret {
            name: name.to_string(),
            label: label.map(str::to_string),
            description: String::new(),
            required,
        }
    }

    fn option_value(
        name: &str,
        label: Option<&str>,
        required: bool,
        value: Option<&str>,
    ) -> BackendOption {
        BackendOption {
            name: name.to_string(),
            label: label.map(str::to_string),
            description: String::new(),
            default: None,
            required,
            value: value.map(str::to_string),
        }
    }

    /// A required, unconfigured secret with a label surfaces with the label
    /// (not the `snake_case` wire name).
    #[test]
    fn required_secret_unconfigured_surfaces_label() {
        let bi = backend(
            vec![secret("openai_api_key", Some("OpenAI API key"), true)],
            Vec::new(),
        );
        let map: HashMap<(String, String), bool> = HashMap::new();

        let missing = unmet_requirements(&map, &bi);
        assert_eq!(missing, vec!["OpenAI API key"]);
    }

    /// A required secret with no label falls back to its `name`. The UI is
    /// then no worse than today but no better — every real backend should
    /// supply a label.
    #[test]
    fn required_secret_without_label_falls_back_to_name() {
        let bi = backend(vec![secret("openai_api_key", None, true)], Vec::new());
        let map: HashMap<(String, String), bool> = HashMap::new();

        let missing = unmet_requirements(&map, &bi);
        assert_eq!(missing, vec!["openai_api_key"]);
    }

    /// A required secret that's marked configured in the keyring map is not
    /// surfaced as unmet — Select must be enabled.
    #[test]
    fn configured_secret_is_not_unmet() {
        let bi = backend(
            vec![secret("openai_api_key", Some("OpenAI API key"), true)],
            Vec::new(),
        );
        let mut map = HashMap::new();
        map.insert((bi.source.clone(), "openai_api_key".to_string()), true);

        assert!(unmet_requirements(&map, &bi).is_empty());
    }

    /// A *non*-required secret never surfaces, configured or not — the daemon
    /// doesn't need it for a load, so it doesn't gate the Select button.
    #[test]
    fn non_required_secret_never_surfaces() {
        let bi = backend(
            vec![secret("openai_org", Some("OpenAI Org"), false)],
            Vec::new(),
        );
        let map = HashMap::new();

        assert!(unmet_requirements(&map, &bi).is_empty());
    }

    /// A required option with no effective value surfaces (its `label`); one
    /// with a value (including a manifest default) does not.
    #[test]
    fn required_option_value_gating() {
        let bi = backend(
            Vec::new(),
            vec![option_value("base_url", Some("Base URL"), true, None)],
        );
        let map = HashMap::new();
        assert_eq!(unmet_requirements(&map, &bi), vec!["Base URL"]);

        let bi_with_value = backend(
            Vec::new(),
            vec![option_value(
                "base_url",
                Some("Base URL"),
                true,
                Some("https://api.openai.com"),
            )],
        );
        assert!(unmet_requirements(&map, &bi_with_value).is_empty());
    }

    /// A whitespace-only value is treated as empty — `value = "   "` does not
    /// satisfy a `required` option.
    #[test]
    fn required_option_whitespace_is_empty() {
        let bi = backend(
            Vec::new(),
            vec![option_value(
                "base_url",
                Some("Base URL"),
                true,
                Some("   "),
            )],
        );
        let map = HashMap::new();
        assert_eq!(unmet_requirements(&map, &bi), vec!["Base URL"]);
    }

    /// Multiple unmet requirements are returned in declaration order
    /// (secrets first, then options) — keeps the inline warnings stable
    /// across renders rather than depending on HashMap iteration order.
    #[test]
    fn returns_in_declaration_order() {
        let bi = backend(
            vec![
                secret("alpha_key", Some("Alpha key"), true),
                secret("beta_key", Some("Beta key"), true),
            ],
            vec![option_value("base_url", Some("Base URL"), true, None)],
        );
        let map = HashMap::new();
        assert_eq!(
            unmet_requirements(&map, &bi),
            vec!["Alpha key", "Beta key", "Base URL"]
        );
    }
}

/// Readiness of the Models page, surfaced as the 4-state status dot in the
/// title row. Strictly ordered from "least ready" to "ready":
/// [`None`](ModelStatus::None) → [`Blocked`](ModelStatus::Blocked) →
/// [`Idle`](ModelStatus::Idle) → [`Ready`](ModelStatus::Ready).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelStatus {
    /// No backend is selected.
    None,
    /// A backend is selected but at least one required secret/option is unset.
    Blocked,
    /// Backend selected and fully configured, but no model is loaded yet.
    Idle,
    /// A model is loaded and ready.
    Ready,
}

/// Compute the [`ModelStatus`] for the current app state — the same rule
/// that drives the in-card warning rows is also what flips the dot's color.
pub(super) fn model_status(app: &AppModel) -> ModelStatus {
    classify_model_status(
        app.active_backend.as_deref(),
        &app.backends,
        &app.backend_secret_configured,
        &app.current_model,
        &app.current_source,
    )
}

/// Pure implementation of [`model_status`] — takes only the inputs the rule
/// depends on so it's directly unit-testable without building an
/// [`AppModel`]. Two arms surface as [`ModelStatus::None`]: there's no
/// active backend at all, OR the daemon reports one but its catalog entry
/// is gone (uninstalled while running). Both are "no backend" from the
/// user's perspective.
pub(super) fn classify_model_status(
    active_backend: Option<&str>,
    backends: &[BackendInfo],
    secret_configured: &std::collections::HashMap<(String, String), bool>,
    current_model: &str,
    current_source: &str,
) -> ModelStatus {
    let Some(active_source) = active_backend else {
        return ModelStatus::None;
    };
    let Some(backend) = backends.iter().find(|b| b.source.as_str() == active_source) else {
        return ModelStatus::None;
    };
    if !unmet_requirements(secret_configured, backend).is_empty() {
        return ModelStatus::Blocked;
    }
    let model_loaded = !current_model.is_empty() && current_source == active_source;
    if model_loaded {
        ModelStatus::Ready
    } else {
        ModelStatus::Idle
    }
}

#[cfg(test)]
mod model_status_tests {
    //! Pin the 4-state status-dot rule that lives in the page header. The
    //! states are: no backend selected → gray; backend selected but
    //! requirements unmet → red; ready but no model loaded → yellow; model
    //! loaded → green.
    use super::*;
    use crate::daemon::backends::{BackendInfo, BackendModel, BackendSecret};
    use std::collections::HashMap;

    fn backend_with_required_secret() -> BackendInfo {
        BackendInfo {
            source: "github.com/super-stt/openai".to_string(),
            name: "OpenAI".to_string(),
            models: vec![BackendModel {
                name: "whisper-1".to_string(),
                provider: "openai".to_string(),
                supported_devices: vec!["none".to_string()],
                estimated_vram_bytes: 0,
            }],
            secrets: vec![BackendSecret {
                name: "openai_api_key".to_string(),
                label: Some("OpenAI API key".to_string()),
                description: String::new(),
                required: true,
            }],
            options: Vec::new(),
        }
    }

    /// No active backend → gray dot regardless of what else is in state.
    #[test]
    fn no_active_backend_is_none() {
        let backends = vec![backend_with_required_secret()];
        let map = HashMap::new();
        assert_eq!(
            classify_model_status(None, &backends, &map, "", ""),
            ModelStatus::None,
        );
    }

    /// Active backend whose catalog entry is gone (e.g. uninstalled while
    /// running) still reads as "no backend" — the daemon's state is stale,
    /// and the dot shouldn't lie about readiness.
    #[test]
    fn active_backend_missing_from_catalog_is_none() {
        let map = HashMap::new();
        assert_eq!(
            classify_model_status(Some("github.com/super-stt/openai"), &[], &map, "", "",),
            ModelStatus::None,
        );
    }

    /// Active backend with an unmet required secret → red, even if a model
    /// from another backend happens to be loaded (which shouldn't really
    /// happen after `set_active_backend` unloads on switch, but the dot
    /// should still reflect the current backend's state).
    #[test]
    fn unmet_requirement_is_blocked() {
        let backends = vec![backend_with_required_secret()];
        let map = HashMap::new();
        assert_eq!(
            classify_model_status(Some("github.com/super-stt/openai"), &backends, &map, "", "",),
            ModelStatus::Blocked,
        );
    }

    /// All requirements satisfied but no model loaded for the active backend
    /// → yellow.
    #[test]
    fn requirements_met_no_model_is_idle() {
        let backends = vec![backend_with_required_secret()];
        let mut map = HashMap::new();
        map.insert(
            (backends[0].source.clone(), "openai_api_key".to_string()),
            true,
        );
        assert_eq!(
            classify_model_status(Some("github.com/super-stt/openai"), &backends, &map, "", "",),
            ModelStatus::Idle,
        );
    }

    /// A loaded model from a *different* source than the active backend is
    /// not "this backend ready" — the dot stays yellow until the user picks
    /// a model from the active backend.
    #[test]
    fn loaded_model_from_other_source_is_idle() {
        let backends = vec![backend_with_required_secret()];
        let mut map = HashMap::new();
        map.insert(
            (backends[0].source.clone(), "openai_api_key".to_string()),
            true,
        );
        assert_eq!(
            classify_model_status(
                Some("github.com/super-stt/openai"),
                &backends,
                &map,
                "voxtral-mini-latest",
                "github.com/super-stt/mistral",
            ),
            ModelStatus::Idle,
        );
    }

    /// Requirements met *and* a model from this backend is loaded → green.
    #[test]
    fn loaded_model_from_active_backend_is_ready() {
        let backends = vec![backend_with_required_secret()];
        let mut map = HashMap::new();
        map.insert(
            (backends[0].source.clone(), "openai_api_key".to_string()),
            true,
        );
        assert_eq!(
            classify_model_status(
                Some("github.com/super-stt/openai"),
                &backends,
                &map,
                "whisper-1",
                "github.com/super-stt/openai",
            ),
            ModelStatus::Ready,
        );
    }
}
