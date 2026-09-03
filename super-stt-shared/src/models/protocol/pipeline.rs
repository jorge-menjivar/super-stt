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

/// One stage of the pipeline: which backend fills it, what is running there,
/// and the load still in flight, if any.
///
/// Every optional field here serializes as an explicit `null` rather than being
/// omitted — a stage reports its whole shape whatever state it is in, so a
/// client can read `source` without first checking the key exists. `enabled` is
/// the one exception, and is absent rather than null on a stage that has no
/// on/off choice of its own.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StageReport {
    /// Position in the pipeline: [`TRANSCRIPTION_STAGE`], [`POST_PROCESSOR_STAGE`].
    pub stage: u32,
    pub role: StageRole,
    /// The backend filling this stage; `null` when the stage is empty.
    pub source: Option<String>,
    /// That backend's display name; `null` when the stage is empty.
    pub name: Option<String>,
    /// The model selected in this stage; `null` when none is picked.
    pub model: Option<String>,
    /// Whether that model is loaded and ready to run.
    pub loaded: bool,
    /// The accelerator the loaded model actually runs on — not the user's
    /// preference, which is read per model through
    /// `/pipeline/{stage}/model/{model}/device`. `null` when nothing is loaded,
    /// since nothing runs anywhere.
    pub device: Option<String>,
    /// The load or download in flight for this stage; `null` when idle.
    ///
    /// The daemon runs one model operation at a time but not always for the
    /// same stage, so this is reported per stage rather than globally.
    pub switch: Option<StageSwitch>,
    /// The user's on/off choice, for stages that carry one separately from
    /// whether the model came up: a stage can be enabled while its load failed,
    /// and transcripts then pass through untouched. Absent on stage 1, which
    /// has no switch of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
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
        POST_PROCESSOR_STAGE, StageReport, StageRole, StageSwitch, SwitchDownload, SwitchTarget,
        TRANSCRIPTION_STAGE,
    };

    fn empty_stage(stage: u32, role: StageRole) -> StageReport {
        StageReport {
            stage,
            role,
            source: None,
            name: None,
            model: None,
            loaded: false,
            device: None,
            switch: None,
            enabled: None,
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

        for key in ["source", "name", "model", "device", "switch"] {
            assert!(object.contains_key(key), "an empty stage omits {key}");
            assert!(object[key].is_null(), "{key} should be null when unset");
        }
        assert_eq!(object["loaded"], false);
    }

    /// `enabled` is the one field that is absent rather than null, and only on
    /// the stage that has no on/off choice of its own.
    ///
    /// The client distinguishes the two: `StageState::is_enabled` falls back to
    /// `loaded` when `enabled` is absent. A stage 1 that started sending
    /// `"enabled": false` would read as "switched off" rather than "has no
    /// switch", and the card would show a running model as disabled.
    #[test]
    fn only_a_stage_with_a_switch_reports_enabled() {
        let stage_one =
            serde_json::to_value(empty_stage(TRANSCRIPTION_STAGE, StageRole::Transcription))
                .expect("serializes");
        assert!(
            stage_one.get("enabled").is_none(),
            "stage 1 has no on/off choice, so it must not report one"
        );

        let mut two = empty_stage(POST_PROCESSOR_STAGE, StageRole::PostProcessor);
        two.enabled = Some(false);
        let stage_two = serde_json::to_value(two).expect("serializes");
        assert_eq!(
            stage_two["enabled"], false,
            "stage 2 carries its choice separately from whether the model came up"
        );
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

    /// A stage survives the round trip whole, switch included.
    ///
    /// `/pipeline/{stage}` narrows one stage out of the array `/pipeline`
    /// returns, so the two views agree only for as long as the type they share
    /// carries everything. A field lost in serialization would take the whole
    /// switch report with it and the download progress a UI polls for.
    #[test]
    fn a_stage_mid_download_round_trips() {
        let report = StageReport {
            stage: POST_PROCESSOR_STAGE,
            role: StageRole::PostProcessor,
            source: Some("github.com/super-stt/s1-mini".to_string()),
            name: Some("S1 Mini".to_string()),
            model: Some("s1-mini-q4_k_m".to_string()),
            loaded: false,
            device: None,
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
            enabled: Some(true),
        };

        let json = serde_json::to_string(&report).expect("serializes");
        let back: StageReport = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, report, "a stage lost something on the wire");

        // The nesting the download poller walks, spelled out: it reads
        // `switch.download.percentage` and `switch.target.model` by name.
        let value: serde_json::Value = serde_json::from_str(&json).expect("parses");
        assert_eq!(value["switch"]["target"]["model"], "s1-mini-q4_k_m");
        assert_eq!(value["switch"]["download"]["percentage"], 25.0);
        assert_eq!(value["switch"]["download"]["eta_seconds"], 30);
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
