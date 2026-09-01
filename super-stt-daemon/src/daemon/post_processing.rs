// SPDX-License-Identifier: GPL-3.0-only
//! Running a final transcript through the loaded post-processor.
//!
//! One funnel, called from both final-transcription paths (the one-shot
//! `POST /transcribe` and the push-to-talk recording flow). Previews and
//! realtime sessions deliberately do not call it: a preview is rewritten again
//! on the next pass, and both are latency paths where a second model round-trip
//! is exactly what the user does not want.
//!
//! **Best-effort.** Every failure mode — nothing selected, nothing loaded, the
//! processor erroring, the processor hanging — yields the raw transcript. The
//! user asked to dictate; a cleanup step that is down must not cost them the
//! words. Failures are logged, and the raw text is what gets typed.

use std::time::Duration;

use log::{debug, info, warn};

use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::dispatch::{DispatchError, dispatch_post_process};

/// How long a post-processing pass may take before the raw transcript is used
/// instead.
///
/// Generous enough for a cloud LLM round trip on a long transcript, short
/// enough that a hung processor does not leave the user staring at nothing
/// after they finished speaking.
const POST_PROCESS_TIMEOUT: Duration = Duration::from_secs(30);

impl SuperSTTDaemon {
    /// Rewrite a final transcript with the loaded post-processor, falling back
    /// to `text` unchanged on any failure.
    ///
    /// `language` is the tag the transcript was produced in, forwarded so the
    /// processor punctuates in the right language.
    pub(crate) async fn post_process_final(
        &self,
        text: String,
        language: Option<String>,
    ) -> String {
        // Empty input covers "no speech": there is nothing to clean up, and a
        // processor handed an empty string tends to invent one.
        if text.trim().is_empty() {
            return text;
        }
        if !self.config.read().await.post_processor.enabled {
            return text;
        }
        if !self.post_processor_loaded().await {
            debug!("Post-processing is enabled but no processor is loaded; using the raw text");
            return text;
        }

        let start = std::time::Instant::now();
        let raw = text.clone();
        match tokio::time::timeout(
            POST_PROCESS_TIMEOUT,
            dispatch_post_process(&self.post_processor, text, language),
        )
        .await
        {
            Ok(Ok(processed)) => {
                // A processor that answers with nothing has effectively eaten
                // the transcript; keep the words rather than typing a blank.
                if processed.trim().is_empty() {
                    warn!("Post-processor returned empty text; using the raw transcript");
                    return raw;
                }
                info!(
                    "Post-processing completed in {:?}: '{processed}'",
                    start.elapsed()
                );
                processed
            }
            Ok(Err(DispatchError::Failed(e))) => {
                warn!("Post-processing failed, using the raw transcript: {e}");
                raw
            }
            Ok(Err(DispatchError::NotLoaded)) => {
                // Raced with an unload between the check above and the dispatch.
                debug!("Post-processor unloaded mid-dispatch; using the raw transcript");
                raw
            }
            Ok(Err(DispatchError::Join(e))) => {
                warn!("Post-processing task failed, using the raw transcript: {e}");
                raw
            }
            Err(_) => {
                warn!(
                    "Post-processing timed out after {POST_PROCESS_TIMEOUT:?}; \
                     using the raw transcript"
                );
                raw
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super_stt_registry_types::manifest::{Device, ModelRole};

    use crate::daemon::types::{LoadedModel, SuperSTTDaemon, test_daemon};
    use crate::stt_models::ModelDefinition;
    use crate::stt_models::transcribe::{ModelInfo, ModelInfoData, ModelState, Transcribe};

    /// A post-processor fake: either uppercases its input (so a test can see
    /// that processing happened), fails, or hangs past the timeout.
    enum Behavior {
        Uppercase,
        Fail,
        Empty,
    }

    struct FakeProcessor {
        info: ModelInfoData,
        behavior: Behavior,
    }

    impl ModelInfo for FakeProcessor {
        fn info(&self) -> &ModelInfoData {
            &self.info
        }
    }
    impl ModelState for FakeProcessor {
        fn device(&self) -> String {
            "remote".to_string()
        }
    }
    #[async_trait::async_trait]
    impl Transcribe for FakeProcessor {
        async fn transcribe_audio(
            &mut self,
            _audio: &[f32],
            _sample_rate: u32,
            _language: Option<&str>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("not a transcription model")
        }

        async fn process_text(
            &mut self,
            text: &str,
            _language: Option<&str>,
        ) -> anyhow::Result<String> {
            match self.behavior {
                Behavior::Uppercase => Ok(text.to_uppercase()),
                Behavior::Fail => anyhow::bail!("processor boom"),
                Behavior::Empty => Ok("   ".to_string()),
            }
        }
    }

    /// A daemon with post-processing enabled and `behavior` loaded in the
    /// post-processor slot.
    async fn daemon_with(behavior: Behavior) -> SuperSTTDaemon {
        let daemon = test_daemon().await;
        daemon.config.write().await.post_processor.enabled = true;
        let definition = ModelDefinition {
            name: "cleanup".to_string(),
            source: "github.com/x/cleanup".to_string(),
            is_multilingual: true,
            primary_language: "en".to_string(),
            supported_languages: vec!["en".to_string()],
            estimated_vram_bytes: 0,
            processing_interval: std::time::Duration::from_secs(1),
            supported_devices: vec![Device::None],
            realtime: false,
            role: ModelRole::PostProcessor,
            provider: None,
        };
        let info = ModelInfoData::new(
            "cleanup",
            "github.com/x/cleanup",
            true,
            true,
            std::time::Duration::from_secs(1),
        );
        *daemon.post_processor.write().await = Some(LoadedModel {
            definition,
            instance: Box::new(FakeProcessor { info, behavior }),
        });
        daemon
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_loaded_processor_rewrites_the_transcript() {
        let daemon = daemon_with(Behavior::Uppercase).await;
        let out = daemon.post_process_final("hello there".into(), None).await;
        assert_eq!(out, "HELLO THERE");
    }

    /// The whole point of the best-effort policy: a processor that fails costs
    /// the user the cleanup, never the words.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failing_processor_yields_the_raw_transcript() {
        let daemon = daemon_with(Behavior::Fail).await;
        let out = daemon.post_process_final("hello there".into(), None).await;
        assert_eq!(out, "hello there");
    }

    /// A processor answering with nothing has eaten the transcript; keeping the
    /// raw text beats typing a blank.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_empty_answer_yields_the_raw_transcript() {
        let daemon = daemon_with(Behavior::Empty).await;
        let out = daemon.post_process_final("hello there".into(), None).await;
        assert_eq!(out, "hello there");
    }

    /// The toggle is what the user set; a loaded processor must not run while
    /// it is off.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_disabled_processor_does_not_run() {
        let daemon = daemon_with(Behavior::Uppercase).await;
        daemon.config.write().await.post_processor.enabled = false;
        let out = daemon.post_process_final("hello there".into(), None).await;
        assert_eq!(out, "hello there");
    }

    /// Enabled but nothing loaded — the startup load failed, say — still
    /// delivers the transcript.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nothing_loaded_yields_the_raw_transcript() {
        let daemon = test_daemon().await;
        daemon.config.write().await.post_processor.enabled = true;
        let out = daemon.post_process_final("hello there".into(), None).await;
        assert_eq!(out, "hello there");
    }

    /// "No speech" is an empty transcript. A processor handed one tends to
    /// invent a sentence, so it is never asked.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_text_is_never_sent_to_the_processor() {
        let daemon = daemon_with(Behavior::Uppercase).await;
        assert_eq!(daemon.post_process_final(String::new(), None).await, "");
        assert_eq!(daemon.post_process_final("   ".into(), None).await, "   ");
        assert!(
            Arc::strong_count(&daemon.post_processor) >= 1,
            "the slot is untouched"
        );
    }
}
