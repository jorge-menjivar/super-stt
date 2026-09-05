// SPDX-License-Identifier: GPL-3.0-only
//! Pipeline stage numbers, as `/pipeline/{stage}` spells them.
//!
//! A transcript passes through ordered stages: stage 1 turns audio into text,
//! stage 2 rewrites it. The number is the address in every pipeline path, the
//! `stage` field on the events a stage emits, and how a client tells whose
//! model a load or a download belongs to — so it is defined once, here, rather
//! than spelled `1` and `2` at each of those sites.

/// Stage 1: audio to text.
pub const TRANSCRIPTION_STAGE: u32 = 1;

/// Stage 2: the transcript rewriter.
pub const POST_PROCESSOR_STAGE: u32 = 2;

/// The stage a payload carrying no `stage` field belongs to.
///
/// Transcription is the only stage that existed before the field, so an older
/// daemon's events and download reports read as stage 1 — which is what they
/// always were. Used as the serde default on every `stage` field.
#[must_use]
pub(crate) const fn default_stage() -> u32 {
    TRANSCRIPTION_STAGE
}

// --- The stage report -------------------------------------------------------
//
// `GET /pipeline` and `GET /pipeline/{stage}` answer with these. They were
// `serde_json::Value` on both ends until the protocol grew a generated OpenAPI
// document: the daemon built each stage with `json!` and `/pipeline/{stage}`
// picked its stage back out of the array with `.get("stage")`, so every field
// name existed only as a string literal at each site and nothing checked that
// the two agreed. A published schema cannot be generated from a `Value`, and
// the alternative — describing the shape a third time, in the spec — is the
// same drift with one more place to forget. So the shape is a type, and the
// spec, the builder and the reader all come off it.

use serde::{Deserialize, Serialize};

/// What a stage does to the transcript passing through it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum StageRole {
    /// Stage 1: audio in, text out.
    Transcription,
    /// A later stage: rewrites the text the stage before it produced.
    PostProcessor,
}

/// One stage of the pipeline: which backend fills it, and whether the user has
/// it switched on.
///
/// A stage reports its *backend*, not its model. The model is one level down,
/// at `GET /pipeline/{stage}/model`, as [`StageModelReport`].
///
/// The two were one object until the stages were made to behave alike, and the
/// split is what fixed them: stage 1 reported the model it had *loaded* while
/// stage 2 reported the model it had *selected*, so the same field meant
/// different things at the two positions — and at stage 1 `loaded` was true
/// exactly when `model` was non-null, carrying no information at all.
///
/// `source` and `name` serialize as an explicit `null` rather than being
/// omitted: a stage reports its whole shape whatever state it is in, so a
/// client can read `source` to decide whether the stage is filled without
/// first checking the key exists.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StageReport {
    /// Position in the pipeline: [`TRANSCRIPTION_STAGE`], [`POST_PROCESSOR_STAGE`].
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
    /// transcripts then pass through untouched. Every stage carries one. Stage
    /// 1 did not until the stages were made to behave alike, which is why its
    /// unload had to throw the selection away to stay idle across a restart.
    pub enabled: bool,
}

/// The model slot of one stage: what is selected, whether it is up, the device
/// it runs on, and the load still in flight.
///
/// Answers `GET /pipeline/{stage}/model`, and answers it the same way at every
/// position — which is the point of it being its own object. `model` is the
/// *selection* and survives an unload; `loaded` says whether that selection is
/// running right now.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StageModelReport {
    /// The stage whose model slot this is.
    pub stage: u32,
    /// The model selected in this stage; `null` when none is picked.
    pub model: Option<String>,
    /// Whether that model is loaded and ready to run.
    pub loaded: bool,
    /// The accelerator the selection runs on; `null` when nothing is selected.
    ///
    /// What it *could* run on is `GET /pipeline/{stage}/model/{model}/device/list`,
    /// kept out of here deliberately: that list costs a host probe, and a card
    /// fills its picker from it once rather than on every poll of this.
    pub device: Option<StageModelDevice>,
    /// The load or download in flight for this stage; `null` when idle.
    ///
    /// Here rather than on the stage because it names a model. The daemon runs
    /// one model operation at a time but not always for the same stage, so it
    /// is reported per stage.
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
        POST_PROCESSOR_STAGE, StageModelDevice, StageModelReport, StageReport, StageRole,
        StageSwitch, SwitchDownload, SwitchTarget, TRANSCRIPTION_STAGE,
    };

    fn empty_stage(stage: u32, role: StageRole) -> StageReport {
        StageReport {
            stage,
            role,
            source: None,
            name: None,
            enabled: false,
        }
    }

    fn empty_model(stage: u32) -> StageModelReport {
        StageModelReport {
            stage,
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
        let json = serde_json::to_value(empty_stage(TRANSCRIPTION_STAGE, StageRole::Transcription))
            .expect("serializes");
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
    /// stage that also carried `model`/`loaded`/`device` answered for two
    /// different lifetimes at once — a durable backend selection and an
    /// ephemeral loaded instance — and the two stages disagreed about which one
    /// `model` meant. Those fields belong to [`StageModelReport`] now, and a
    /// client reading them off a stage would silently get `null` forever.
    #[test]
    fn a_stage_carries_no_model_fields() {
        let mut report = empty_stage(TRANSCRIPTION_STAGE, StageRole::Transcription);
        report.source = Some("github.com/acme/whisper".to_string());
        report.name = Some("Whisper".to_string());
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

    /// Every stage reports `enabled`, at every position.
    ///
    /// Stage 1 used to omit it, meaning "has no on/off choice of its own", and
    /// the client's `is_enabled()` fell back to `loaded`. That fallback is what
    /// made stage 1 unable to hold a selection it was not running — and so what
    /// made its unload throw the selection away. Both stages carry the field
    /// now, and an absent one would resurrect the fallback.
    #[test]
    fn every_stage_reports_whether_it_is_switched_on() {
        for (stage, role) in [
            (TRANSCRIPTION_STAGE, StageRole::Transcription),
            (POST_PROCESSOR_STAGE, StageRole::PostProcessor),
        ] {
            let mut report = empty_stage(stage, role);
            report.enabled = true;
            let json = serde_json::to_value(report).expect("serializes");
            assert_eq!(
                json["enabled"], true,
                "stage {stage} must report its on/off state"
            );
        }
    }

    /// The role crosses the wire in `snake_case`, not as its Rust name.
    #[test]
    fn roles_use_their_wire_spelling() {
        let json =
            serde_json::to_value(empty_stage(POST_PROCESSOR_STAGE, StageRole::PostProcessor))
                .expect("serializes");
        assert_eq!(json["role"], "post_processor");
        assert_eq!(
            serde_json::to_value(StageRole::Transcription).expect("serializes"),
            "transcription"
        );
    }

    /// A model slot with nothing in it reports nulls, not absent keys — the
    /// same contract the stage keeps, and for the same reason.
    #[test]
    fn an_empty_model_slot_reports_nulls_rather_than_absent_keys() {
        let json = serde_json::to_value(empty_model(TRANSCRIPTION_STAGE)).expect("serializes");
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
    /// This is the state stage 1 could not represent at all: its `model` came
    /// off the loaded instance, so the two fields were one bit. It is what a
    /// card shows after an unload, and what makes re-loading the same model on
    /// a different device a single choice rather than a re-selection.
    #[test]
    fn a_selection_survives_without_being_loaded() {
        let mut slot = empty_model(TRANSCRIPTION_STAGE);
        slot.model = Some("whisper-large-v3".to_string());
        slot.device = Some(StageModelDevice {
            preference: "gpu".to_string(),
            resolved_accel: None,
        });

        let json = serde_json::to_value(&slot).expect("serializes");
        assert_eq!(json["model"], "whisper-large-v3");
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
    /// The switch moved here from the stage when the two split, so this is what
    /// the download poller now reads: a field lost in serialization would take
    /// the whole progress report with it.
    #[test]
    fn a_model_slot_mid_download_round_trips() {
        let slot = StageModelReport {
            stage: POST_PROCESSOR_STAGE,
            model: Some("s1-mini-q4_k_m".to_string()),
            loaded: false,
            device: Some(StageModelDevice {
                preference: "cpu".to_string(),
                resolved_accel: Some("cpu".to_string()),
            }),
            switch: Some(StageSwitch {
                phase: "downloading".to_string(),
                target: SwitchTarget {
                    model: "s1-mini-q4_k_m".to_string(),
                    source: "github.com/super-stt/s1-mini".to_string(),
                },
                started_at: "2026-09-03T12:00:00Z".to_string(),
                download: SwitchDownload {
                    current_file: "model.gguf".to_string(),
                    file_index: 1,
                    total_files: 3,
                    bytes_downloaded: 1024,
                    total_bytes: 4096,
                    percentage: 25.0,
                    eta_seconds: Some(30),
                },
            }),
        };

        let json = serde_json::to_string(&slot).expect("serializes");
        let back: StageModelReport = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, slot, "a model slot lost something on the wire");

        // The nesting the download poller walks, spelled out: it reads
        // `switch.download.percentage` and `switch.target.model` by name.
        let value: serde_json::Value = serde_json::from_str(&json).expect("parses");
        assert_eq!(value["switch"]["target"]["model"], "s1-mini-q4_k_m");
        assert_eq!(value["switch"]["download"]["percentage"], 25.0);
        assert_eq!(value["switch"]["download"]["eta_seconds"], 30);
    }

    /// A stage's very first load is in flight before anything is selected, so
    /// the switch has to be readable while `model` is still `null`.
    #[test]
    fn a_first_load_reports_its_switch_before_there_is_a_selection() {
        let mut slot = empty_model(TRANSCRIPTION_STAGE);
        slot.switch = Some(StageSwitch {
            phase: "loading_model".to_string(),
            target: SwitchTarget {
                model: "whisper-tiny".to_string(),
                source: "github.com/acme/whisper".to_string(),
            },
            started_at: "2026-09-03T12:00:00Z".to_string(),
            download: SwitchDownload {
                current_file: String::new(),
                file_index: 0,
                total_files: 0,
                bytes_downloaded: 0,
                total_bytes: 0,
                percentage: 100.0,
                eta_seconds: None,
            },
        });
        let json = serde_json::to_value(slot).expect("serializes");
        assert!(json["model"].is_null());
        assert_eq!(json["switch"]["target"]["model"], "whisper-tiny");
    }

    /// An estimate the daemon cannot make yet is `null`, not a zero that would
    /// render as "0 seconds remaining".
    #[test]
    fn an_unknown_eta_is_null() {
        let download = SwitchDownload {
            current_file: "model.gguf".to_string(),
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
