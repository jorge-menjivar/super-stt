// SPDX-License-Identifier: GPL-3.0-only
//! `Transcribe` + `ModelInfo` wrapper for [`OpenAIModel`].

use anyhow::Result;
use async_trait::async_trait;
use candle_core::Device;
use std::time::Duration;
use super_stt_shared::models::provider::Provider;

use super::model::OpenAIModel;
use crate::stt_models::transcribe::{ModelInfo, ModelInfoData, Transcribe};

/// OpenAI client paired with its identity metadata.
pub struct OpenAIEntry {
    inner: OpenAIModel,
    info: ModelInfoData,
}

impl OpenAIEntry {
    pub fn new(api_key: String, model_id: String, info: ModelInfoData) -> Self {
        Self {
            inner: OpenAIModel::new(api_key, model_id),
            info,
        }
    }
}

impl ModelInfo for OpenAIEntry {
    fn provider(&self) -> Provider {
        self.info.provider
    }
    fn display_name(&self) -> &str {
        &self.info.display_name
    }
    fn is_multilingual(&self) -> bool {
        self.info.is_multilingual
    }
    fn requires_gpu(&self) -> bool {
        self.info.requires_gpu
    }
    fn processing_interval(&self) -> Duration {
        self.info.processing_interval
    }
}

#[async_trait]
impl Transcribe for OpenAIEntry {
    async fn transcribe_audio(&mut self, audio: &[f32], sample_rate: u32) -> Result<String> {
        OpenAIModel::transcribe_audio(&self.inner, audio, sample_rate).await
    }

    fn device(&self) -> &Device {
        // Online models don't run on a local device — return CPU as a placeholder.
        &Device::Cpu
    }
}
