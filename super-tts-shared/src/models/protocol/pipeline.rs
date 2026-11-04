// SPDX-License-Identifier: GPL-3.0-only
//! Pipeline stage numbers, as `/pipeline/{stage}` spells them.
//!
//! An utterance passes through ordered stages. Super TTS has one today —
//! stage 1 turns text into audio — and addresses it by position anyway, so a
//! stage added ahead of it (a text normalizer, an LLM rewriter) is a new row
//! rather than a new endpoint family. The number is the address in every
//! pipeline path and how a client tells whose model a load or a download
//! belongs to, so it is defined once, here, rather than spelled `1` at each of
//! those sites.

/// Stage 1: text to audio.
pub const SYNTHESIS_STAGE: u32 = 1;

/// The stage a payload carrying no `stage` field belongs to.
///
/// Synthesis is the only stage there has ever been, so a payload from a daemon
/// that predates the field reads as stage 1 — which is what it always was.
/// Used as the serde default on every `stage` field.
#[must_use]
pub const fn default_stage() -> u32 {
    SYNTHESIS_STAGE
}

// --- The stage report -------------------------------------------------------
//
// `GET /pipeline` and `GET /pipeline/{stage}` answer with these. They would
// otherwise be `serde_json::Value` on both ends: the daemon building each stage
// with `json!` and the endpoint picking its stage back out of the array with
// `.get("stage")`, so every field name exists only as a string literal at each
// site and nothing checks that the two agree. A published schema cannot be
// generated from a `Value`, and the alternative — describing the shape a third
// time, in the spec — is the same drift with one more place to forget. So the
// shape is a type, and the spec, the builder and the reader all come off it.

use serde::{Deserialize, Serialize};

/// What a stage does to the utterance passing through it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum StageRole {
    /// Stage 1: text in, audio out.
    Synthesis,
}

/// One stage of the pipeline: which backend fills it, and whether the user has
/// it switched on.
///
/// A stage reports its *backend*, not its model. The model is one level down,
/// at `GET /pipeline/{stage}/model`, as [`StageModelReport`]. The split is what
/// keeps the two lifetimes apart: a backend selection is durable and cannot
/// fail for runtime reasons, while a model load downloads, allocates, and can.
/// Reporting them together is what lets a client confuse "no backend chosen"
/// with "chosen, but its model is not up".
///
/// `source` and `name` serialize as an explicit `null` rather than being
/// omitted: a stage reports its whole shape whatever state it is in, so a
/// client can read `source` to decide whether the stage is filled without
/// first checking the key exists.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StageReport {
    /// Position in the pipeline: [`SYNTHESIS_STAGE`].
    pub stage: u32,
    pub role: StageRole,
    /// The backend filling this stage; `null` when the stage is empty.
    pub source: Option<String>,
    /// That backend's display name; `null` when the stage is empty.
    pub name: Option<String>,
    /// Whether the user has this stage switched on — what Load sets and Unload
    /// clears.
    ///
    /// Separate from whether the model actually came up, which is `loaded` on
    /// [`StageModelReport`]: a stage can be enabled while its load failed, and
    /// nothing is spoken until it succeeds.
    pub enabled: bool,
}

/// The model slot of one stage: what is selected, whether it is up, the device
/// it runs on, and the load still in flight.
///
/// Answers `GET /pipeline/{stage}/model`. `model` is the *selection* and
/// survives an unload; `loaded` says whether that selection is running right
/// now. Keeping them distinct is what lets a card show "Kokoro, unloaded" and
/// re-load it onto a different device as one choice rather than a
/// re-selection.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StageModelReport {
    /// The stage whose model slot this is.
    pub stage: u32,
    /// The model selected in this stage; `null` when none is picked.
    pub model: Option<String>,
    /// Whether that model is loaded and ready to synthesize.
    pub loaded: bool,
    /// The accelerator the selection runs on; `null` when nothing is selected.
    ///
    /// What it *could* run on is `GET /pipeline/{stage}/model/{model}/device/list`,
    /// kept out of here deliberately: that list costs a host probe, and a card
    /// fills its picker from it once rather than on every poll of this.
    pub device: Option<StageModelDevice>,
    /// The load or download in flight for this stage; `null` when idle.
    ///
    /// Here rather than on the stage because it names a model.
    pub switch: Option<StageSwitch>,
}

/// Which accelerator a stage's model runs on.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StageModelDevice {
    /// The stored preference: `cpu`, `gpu`, or `none` for a model that runs
    /// remotely and therefore has no local device.
    pub preference: String,
    /// What a `gpu` preference resolved to once the model loaded — `cuda`,
    /// `rocm`, `metal`, `vulkan`. `null` while the preference is `gpu` and
    /// nothing has confirmed it yet, so a client is never told a device
    /// resolved before a load proved it.
    pub resolved_accel: Option<String>,
}

/// A model load in flight for one stage.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StageSwitch {
    /// Where the operation has got to: `downloading`, `loading_model`,
    /// `cancelled`, `completed`, or `error`.
    pub phase: String,
    /// The model being loaded into the stage.
    pub target: SwitchTarget,
    /// RFC 3339 timestamp of when the operation started.
    pub started_at: String,
    /// Byte and file progress, for the `downloading` phase.
    pub download: SwitchDownload,
    /// The backend's own account of the load, in the `loading_model` phase:
    /// its phase, its step, and how far through the step it is. Absent until
    /// the backend reports any of it, and from backends that never do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<super::LoadProgress>,
}

/// The model a [`StageSwitch`] is loading.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SwitchTarget {
    pub model: String,
    pub source: String,
}

/// Download progress within a [`StageSwitch`].
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SwitchDownload {
    pub current_file: String,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    /// 0.0–100.0 across the whole operation.
    pub percentage: f32,
    /// Estimated seconds remaining, or `null` before there is enough history
    /// to estimate one.
    pub eta_seconds: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::{
        SYNTHESIS_STAGE, StageModelDevice, StageModelReport, StageReport, StageRole, StageSwitch,
        SwitchDownload, SwitchTarget,
    };

    fn empty_stage() -> StageReport {
        StageReport {
            stage: SYNTHESIS_STAGE,
            role: StageRole::Synthesis,
            source: None,
            name: None,
            enabled: false,
        }
    }

    fn empty_model() -> StageModelReport {
        StageModelReport {
            stage: SYNTHESIS_STAGE,
            model: None,
            loaded: false,
            device: None,
            switch: None,
        }
    }

    /// An empty stage reports every field it has, as an explicit `null`.
    ///
    /// This is deliberate and load-bearing: a client reads `source` to decide
    /// whether a stage is filled, and it can only do that without first
    /// checking the key exists if the key is always there. Adding
    /// `skip_serializing_if` to these fields would be invisible in Rust and
    /// would quietly change that contract.
    #[test]
    fn an_empty_stage_reports_nulls_rather_than_absent_keys() {
        let json = serde_json::to_value(empty_stage()).expect("serializes");
        let object = json.as_object().expect("a stage is an object");

        for key in ["source", "name"] {
            assert!(object.contains_key(key), "an empty stage omits {key}");
            assert!(object[key].is_null(), "{key} should be null when unset");
        }
        assert_eq!(object["enabled"], false);
    }

    /// A stage reports its backend and nothing about its model.
    ///
    /// The regression this pins is the shape the split exists to prevent: a
    /// stage that also carried `model`/`loaded`/`device` would answer for two
    /// different lifetimes at once — a durable backend selection and an
    /// ephemeral loaded instance. Those fields belong to [`StageModelReport`],
    /// and a client reading them off a stage would silently get `null` forever.
    #[test]
    fn a_stage_carries_no_model_fields() {
        let mut report = empty_stage();
        report.source = Some("github.com/acme/kokoro".to_string());
        report.name = Some("Kokoro".to_string());
        let json = serde_json::to_value(report).expect("serializes");
        let object = json.as_object().expect("a stage is an object");

        for key in ["model", "loaded", "device", "switch"] {
            assert!(
                !object.contains_key(key),
                "{key} belongs to /pipeline/{{stage}}/model, not to the stage"
            );
        }
        // Sorted: key order depends on whether `serde_json/preserve_order` is
        // unified in by another crate in the build. The claim is which keys
        // exist, not the order they serialize in.
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["enabled", "name", "role", "source", "stage"]);
    }

    /// The role crosses the wire in `snake_case`, not as its Rust name.
    ///
    /// One variant today, so this looks trivial; it is here because the second
    /// one will be a compound name where the two spellings diverge, and the
    /// wire form is the one clients switch on.
    #[test]
    fn roles_use_their_wire_spelling() {
        assert_eq!(
            serde_json::to_value(StageRole::Synthesis).expect("serializes"),
            "synthesis"
        );
    }

    /// A model slot with nothing in it reports nulls, not absent keys — the
    /// same contract the stage keeps, and for the same reason.
    #[test]
    fn an_empty_model_slot_reports_nulls_rather_than_absent_keys() {
        let json = serde_json::to_value(empty_model()).expect("serializes");
        let object = json.as_object().expect("a model slot is an object");

        for key in ["model", "device", "switch"] {
            assert!(object.contains_key(key), "an empty model slot omits {key}");
            assert!(object[key].is_null(), "{key} should be null when unset");
        }
        assert_eq!(object["loaded"], false);
    }

    /// `model` is the selection and `loaded` is whether it is running, so the
    /// pair "selected but not loaded" has to be expressible.
    ///
    /// It is what a card shows after an unload, and what makes re-loading the
    /// same model on a different device a single choice rather than a
    /// re-selection. Collapsing the two into one bit is what makes an unload
    /// have to throw the selection away to stay idle across a restart.
    #[test]
    fn a_selection_survives_without_being_loaded() {
        let mut slot = empty_model();
        slot.model = Some("kokoro-82m".to_string());
        slot.device = Some(StageModelDevice {
            preference: "gpu".to_string(),
            resolved_accel: None,
        });

        let json = serde_json::to_value(&slot).expect("serializes");
        assert_eq!(json["model"], "kokoro-82m");
        assert_eq!(json["loaded"], false);
        assert_eq!(json["device"]["preference"], "gpu");
        // Nothing has loaded, so nothing has resolved the generic `gpu` yet —
        // reporting one here would name an accelerator no load has confirmed.
        assert!(json["device"]["resolved_accel"].is_null());

        let back: StageModelReport = serde_json::from_value(json).expect("deserializes");
        assert_eq!(back, slot);
    }

    /// The device block carries the preference and what it resolved to, and
    /// nothing else.
    ///
    /// `available_devices` is deliberately not here: it costs a host probe, and
    /// `GET /pipeline/{stage}/model/{model}/device/list` is the endpoint that
    /// answers it. Adding it back would make every poll of a card's model
    /// re-detect the host's GPUs.
    #[test]
    fn the_device_block_does_not_carry_the_device_list() {
        let json = serde_json::to_value(StageModelDevice {
            preference: "cpu".to_string(),
            resolved_accel: Some("cpu".to_string()),
        })
        .expect("serializes");
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["preference", "resolved_accel"]);
    }

    /// A model slot mid-download round trips whole, switch included.
    ///
    /// This is what the download poller reads: a field lost in serialization
    /// would take the whole progress report with it.
    #[test]
    fn a_model_slot_mid_download_round_trips() {
        let slot = StageModelReport {
            stage: SYNTHESIS_STAGE,
            model: Some("kokoro-82m".to_string()),
            loaded: false,
            device: Some(StageModelDevice {
                preference: "cpu".to_string(),
                resolved_accel: Some("cpu".to_string()),
            }),
            switch: Some(StageSwitch {
                phase: "downloading".to_string(),
                target: SwitchTarget {
                    model: "kokoro-82m".to_string(),
                    source: "github.com/super-tts/kokoro".to_string(),
                },
                started_at: "2026-09-07T12:00:00Z".to_string(),
                download: SwitchDownload {
                    current_file: "kokoro-v1_0.pth".to_string(),
                    file_index: 1,
                    total_files: 3,
                    bytes_downloaded: 1024,
                    total_bytes: 4096,
                    percentage: 25.0,
                    eta_seconds: Some(30),
                },
                load: None,
            }),
        };

        let json = serde_json::to_string(&slot).expect("serializes");
        let back: StageModelReport = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, slot, "a model slot lost something on the wire");

        // The nesting the download poller walks, spelled out: it reads
        // `switch.download.percentage` and `switch.target.model` by name.
        let value: serde_json::Value = serde_json::from_str(&json).expect("parses");
        assert_eq!(value["switch"]["target"]["model"], "kokoro-82m");
        assert_eq!(value["switch"]["download"]["percentage"], 25.0);
        assert_eq!(value["switch"]["download"]["eta_seconds"], 30);
    }

    /// A stage's very first load is in flight before anything is selected, so
    /// the switch has to be readable while `model` is still `null`.
    #[test]
    fn a_first_load_reports_its_switch_before_there_is_a_selection() {
        let mut slot = empty_model();
        slot.switch = Some(StageSwitch {
            phase: "loading_model".to_string(),
            target: SwitchTarget {
                model: "kokoro-82m".to_string(),
                source: "github.com/acme/kokoro".to_string(),
            },
            started_at: "2026-09-07T12:00:00Z".to_string(),
            download: SwitchDownload {
                current_file: String::new(),
                file_index: 0,
                total_files: 0,
                bytes_downloaded: 0,
                total_bytes: 0,
                percentage: 100.0,
                eta_seconds: None,
            },
            load: None,
        });
        let json = serde_json::to_value(slot).expect("serializes");
        assert!(json["model"].is_null());
        assert_eq!(json["switch"]["target"]["model"], "kokoro-82m");
    }

    /// An estimate the daemon cannot make yet is `null`, not a zero that would
    /// render as "0 seconds remaining".
    #[test]
    fn an_unknown_eta_is_null() {
        let download = SwitchDownload {
            current_file: "kokoro-v1_0.pth".to_string(),
            file_index: 0,
            total_files: 1,
            bytes_downloaded: 0,
            total_bytes: 4096,
            percentage: 0.0,
            eta_seconds: None,
        };
        let json = serde_json::to_value(download).expect("serializes");
        assert!(json["eta_seconds"].is_null());
    }
}
