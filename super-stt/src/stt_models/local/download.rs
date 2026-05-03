// SPDX-License-Identifier: GPL-3.0-only
use crate::download_progress::DownloadProgressTracker;
use anyhow::{Context as _, Result};
use futures::StreamExt;
use log::{debug, info, warn};
use ring::digest::{Context, SHA256};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::registry::ModelDefinition;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Get the `HuggingFace` Hub URL for a model file
fn get_hf_url(model_id: &str, revision: &str, filename: &str) -> String {
    format!("https://huggingface.co/{model_id}/resolve/{revision}/{filename}")
}

/// Get the cache paths for the Hugging Face-like cache layout.
/// Returns the symlink path under `snapshots/<revision>/<filename>` and the `blobs` directory path.
fn get_cache_paths(model_id: &str, revision: &str, filename: &str) -> Result<(PathBuf, PathBuf)> {
    // Get HF cache directory
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine cache directory"))?
        .join("huggingface")
        .join("hub");

    // Build the model folder path
    let model_folder = format!("models--{}", model_id.replace('/', "--"));
    let snapshot_dir = cache_dir
        .join(&model_folder)
        .join("snapshots")
        .join(revision);

    // The symlink path (what the user sees)
    let symlink_path = snapshot_dir.join(filename);

    // The actual blob storage directory
    let blobs_dir = cache_dir.join(&model_folder).join("blobs");

    Ok((symlink_path, blobs_dir))
}

/// Async function to download a file with progress tracking and cancellation support
async fn cancellable_download(
    model_id: &str,
    revision: &str,
    filename: &str,
    tracker: Arc<DownloadProgressTracker>,
    file_index: usize,
) -> Result<Option<PathBuf>> {
    // Update progress for this file
    tracker.file_index.store(file_index, Ordering::Relaxed);
    tracker.start_file(filename, file_index);

    // Broadcast progress update
    let tracker_clone = Arc::clone(&tracker);
    tokio::spawn(async move {
        tracker_clone.broadcast_progress().await;
    });

    // Check for cancellation before starting download
    if tracker.is_cancelled() {
        warn!("Download cancelled before starting file {filename}");
        return Err(anyhow::anyhow!("Download was cancelled"));
    }

    // Get the cache paths (symlink and blobs directory)
    let (symlink_path, blobs_dir) = get_cache_paths(model_id, revision, filename)?;

    // Check if file already exists and is valid
    if symlink_path.exists() {
        // If the symlink exists, assume cached; optional: verify target exists
        info!("File already cached: {filename}");
        tracker.file_index.store(file_index + 1, Ordering::Relaxed);
        return Ok(Some(symlink_path));
    }

    // Build the download URL
    let url = get_hf_url(model_id, revision, filename);

    // Ensure blobs directory exists
    fs::create_dir_all(&blobs_dir).await?;

    // Download into blobs directory, compute SHA-256, and finalize to blobs/<sha256>
    let final_blob_path =
        download_and_hash_with_cancellation(&url, &blobs_dir, Arc::clone(&tracker)).await?;

    // Create the snapshot directory for the symlink
    if let Some(parent) = symlink_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    // Create a relative symlink from the snapshot to the blob
    let blob_relative_path = {
        // Calculate relative path from snapshot to blob directory
        let mut relative = PathBuf::new();

        // Go up from snapshots/{revision} to the model root
        relative.push("..");
        relative.push("..");

        // Then go to blobs/{hash}
        relative.push("blobs");
        relative.push(final_blob_path.file_name().unwrap());

        relative
    };

    // Create the symlink
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        if let Err(e) = symlink(&blob_relative_path, &symlink_path) {
            warn!("Failed to create symlink for {filename}: {e}");
            // Fall back to returning the blob path directly
            tracker.file_index.store(file_index + 1, Ordering::Relaxed);
            return Ok(Some(final_blob_path));
        }
    }

    #[cfg(not(unix))]
    {
        // On non-Unix systems, just return the blob path
        warn!("Symlinks not supported on this platform, using blob path directly");
        tracker.file_index.store(file_index + 1, Ordering::Relaxed);
        return Ok(Some(final_blob_path));
    }

    // Update file index to show progress
    tracker.file_index.store(file_index + 1, Ordering::Relaxed);

    // Broadcast final progress update
    let tracker_clone = Arc::clone(&tracker);
    tokio::spawn(async move {
        tracker_clone.broadcast_progress().await;
    });

    info!("Successfully downloaded and symlinked: {filename}");
    Ok(Some(symlink_path))
}

/// Download to a temp file in `blobs_dir`, compute SHA-256 while streaming, and finalize to `blobs/<sha256>`
async fn download_and_hash_with_cancellation(
    url: &str,
    blobs_dir: &Path,
    tracker: Arc<DownloadProgressTracker>,
) -> Result<PathBuf> {
    debug!(
        "Starting download with hashing from {url} into {}",
        blobs_dir.display()
    );

    crate::install_crypto_provider();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_mins(5))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()?;

    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Download failed with status {}: {}",
            response.status(),
            url
        ));
    }

    if let Some(size) = response.content_length() {
        tracker.total_bytes.store(size, Ordering::Relaxed);
    }

    // Prepare temp file path
    let tmp_name = format!(".tmp-{}", uuid::Uuid::new_v4());
    let temp_path = blobs_dir.join(tmp_name);
    let mut file = fs::File::create(&temp_path).await?;
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();
    let mut hasher = Context::new(&SHA256);

    while let Some(chunk_result) = stream.next().await {
        if tracker.is_cancelled() {
            drop(file);
            let _ = fs::remove_file(&temp_path).await;
            warn!(
                "Download cancelled, cleaned up temp file: {}",
                temp_path.display()
            );
            return Err(anyhow::anyhow!("Download was cancelled"));
        }

        let chunk = chunk_result?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;

        downloaded += chunk.len() as u64;
        tracker
            .bytes_downloaded
            .store(downloaded, Ordering::Relaxed);

        if downloaded.is_multiple_of(1024 * 1024) {
            let tracker_clone = Arc::clone(&tracker);
            tokio::spawn(async move {
                tracker_clone.broadcast_progress().await;
            });
        }
    }

    file.flush().await?;
    file.sync_all().await?;
    drop(file);

    // Compute final SHA-256 hex
    let digest = hasher.finish();
    let hash_hex = digest.as_ref().iter().fold(String::new(), |mut output, b| {
        let _ = write!(output, "{b:02x}");
        output
    });
    let final_path = blobs_dir.join(hash_hex);

    // If the final blob already exists, discard temp; else move temp into place
    match fs::metadata(&final_path).await {
        Ok(md) if md.len() > 0 => {
            let _ = fs::remove_file(&temp_path).await;
        }
        _ => {
            // Ensure parent exists and rename
            if let Some(parent) = final_path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::rename(&temp_path, &final_path).await?;
        }
    }

    debug!("Download completed and stored at {}", final_path.display());
    Ok(final_path)
}

/// Download model files with progress tracking.
///
/// # Errors
///
/// Returns an error if any file download, file system operation, or progress
/// update fails.
pub async fn with_progress(
    def: &ModelDefinition,
    tracker: Arc<DownloadProgressTracker>,
) -> Result<()> {
    let super_stt_shared::models::registry::ModelSource::HuggingFace { repo, revision, .. } =
        &def.source
    else {
        anyhow::bail!("{}: not a downloadable HuggingFace model", def.name);
    };

    let files = model_filenames(def);

    tracker.total_files.store(files.len(), Ordering::Relaxed);
    tracker.broadcast_progress().await;

    for (index, filename) in files.iter().enumerate() {
        if tracker.is_cancelled() {
            return Err(anyhow::anyhow!("Download was cancelled"));
        }
        cancellable_download(repo, revision, filename, Arc::clone(&tracker), index).await?;
    }

    tracker.mark_completed();
    tracker.broadcast_progress().await;
    Ok(())
}

/// File list for a built-in model. Voxtral families ship multi-shard
/// safetensors; everything else uses a single `model.safetensors`.
fn model_filenames(def: &ModelDefinition) -> Vec<&'static str> {
    let mut files = if def.provider == Provider::LocalVoxtral {
        vec!["config.json", "tekken.json"]
    } else {
        vec!["config.json", "tokenizer.json"]
    };

    let safetensors_files: Vec<&'static str> = match def.name.as_ref() {
        "voxtral-mini" => vec![
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
        ],
        "voxtral-small" => vec![
            "model-00001-of-00011.safetensors",
            "model-00002-of-00011.safetensors",
            "model-00003-of-00011.safetensors",
            "model-00004-of-00011.safetensors",
            "model-00005-of-00011.safetensors",
            "model-00006-of-00011.safetensors",
            "model-00007-of-00011.safetensors",
            "model-00008-of-00011.safetensors",
            "model-00009-of-00011.safetensors",
            "model-00010-of-00011.safetensors",
            "model-00011-of-00011.safetensors",
        ],
        _ => vec!["model.safetensors"],
    };

    files.extend(safetensors_files);
    files
}

/// Get file paths for a built-in model from the `HuggingFace` cache.
///
/// # Errors
///
/// Returns an error if any expected file is not found in the cache.
pub fn get_model_file_paths(def: &ModelDefinition) -> Result<Vec<PathBuf>> {
    let files = model_filenames(def);
    let super_stt_shared::models::registry::ModelSource::HuggingFace { repo, revision, .. } =
        &def.source
    else {
        anyhow::bail!("{}: not a local HuggingFace model", def.name);
    };

    let mut file_paths = Vec::new();
    for filename in &files {
        let (symlink_path, _blob_path) = get_cache_paths(repo, revision, filename)?;
        if symlink_path.exists() {
            file_paths.push(symlink_path);
        } else {
            return Err(anyhow::anyhow!("Model file not found: {filename}"));
        }
    }

    if let Some(first) = file_paths.first().and_then(|p| p.parent()) {
        info!("Using model from cache: {}", first.display());
    }

    Ok(file_paths)
}

// ── Custom model support ────────────────────────────────────────────

/// Re-exports of types that moved to `super-stt-shared` so existing call
/// sites keep working without import churn.
pub use super_stt_shared::models::registry::CustomModelInfo;

/// Detect the architecture of a model from its `config.json`.
///
/// Reads the standard `HuggingFace` `architectures` field and matches each entry
/// against the class names declared by every entry in
/// [`registry::ALL`](super_stt_shared::models::registry::ALL).
/// Adding a new `ModelDefinition` is enough — this function does not need updating.
///
/// # Errors
///
/// Returns an error if `config.json` cannot be read, cannot be parsed, or
/// declares no architecture matching a supported family.
pub fn detect_provider(model_dir: &Path) -> Result<Provider> {
    let config_path = model_dir.join("config.json");
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Cannot read {}", config_path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&content).context("Invalid config.json")?;

    let architectures = json
        .get("architectures")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            anyhow::anyhow!("config.json has no `architectures` field; not a model config")
        })?;

    for arch in architectures.iter().filter_map(|v| v.as_str()) {
        if let Some(provider) = super_stt_shared::models::registry::provider_from_hf_class(arch) {
            return Ok(provider);
        }
    }

    anyhow::bail!("config.json declares no supported architecture")
}

/// Collect all file paths inside a custom model directory.
///
/// Returns every file that the model loaders need: `config.json`, the
/// tokenizer file, and all `.safetensors` weight files.
///
/// # Errors
///
/// Returns an error if `config.json` is missing or the directory cannot be read.
pub fn get_custom_model_file_paths(model_dir: &Path) -> Result<Vec<PathBuf>> {
    let config = model_dir.join("config.json");
    if !config.exists() {
        anyhow::bail!("config.json not found in {}", model_dir.display());
    }

    let mut paths = vec![config];

    // Tokenizer: tekken.json for Voxtral, tokenizer.json for Whisper
    let tekken = model_dir.join("tekken.json");
    let tokenizer = model_dir.join("tokenizer.json");
    if tekken.exists() {
        paths.push(tekken);
    } else if tokenizer.exists() {
        paths.push(tokenizer);
    }

    // Safetensors weight files
    for entry in std::fs::read_dir(model_dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("safetensors") {
            paths.push(p);
        }
    }

    Ok(paths)
}

/// Scan `custom_models_dir` for model subdirectories and return their metadata.
///
/// A subdirectory is recognised as a model when it contains `config.json`.
#[must_use]
pub fn discover_custom_models(custom_models_dir: &Path) -> Vec<CustomModelInfo> {
    let entries = match std::fs::read_dir(custom_models_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(
                "Cannot read custom models directory {}: {e}",
                custom_models_dir.display()
            );
            return Vec::new();
        }
    };

    let mut models = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !path.join("config.json").exists() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        match detect_provider(&path) {
            Ok(provider) => {
                info!("Discovered custom model: {name} ({provider})");
                models.push(CustomModelInfo {
                    name,
                    path,
                    provider,
                });
            }
            Err(e) => {
                debug!("Skipping {}: {e}", path.display());
            }
        }
    }

    models
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("super-stt-tests").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── Architecture detection ──────────────────────────────────────

    #[test]
    fn detect_whisper_architecture() {
        let tmp = test_dir("detect_whisper");
        fs::write(
            tmp.join("config.json"),
            r#"{"architectures": ["WhisperForConditionalGeneration"]}"#,
        )
        .unwrap();
        assert_eq!(detect_provider(&tmp).unwrap(), Provider::LocalWhisper);
    }

    #[test]
    fn detect_voxtral_architecture() {
        let tmp = test_dir("detect_voxtral");
        fs::write(
            tmp.join("config.json"),
            r#"{"architectures": ["VoxtralForConditionalGeneration"]}"#,
        )
        .unwrap();
        assert_eq!(detect_provider(&tmp).unwrap(), Provider::LocalVoxtral);
    }

    #[test]
    fn detect_architecture_missing_config_fails() {
        let tmp = test_dir("detect_missing");
        assert!(detect_provider(&tmp).is_err());
    }

    #[test]
    fn detect_unrelated_config_fails() {
        // e.g. ~/.docker/config.json — has config.json but no `architectures`
        let tmp = test_dir("detect_unrelated");
        fs::write(tmp.join("config.json"), r#"{"auths": {}}"#).unwrap();
        assert!(detect_provider(&tmp).is_err());
    }

    #[test]
    fn detect_unsupported_architecture_fails() {
        let tmp = test_dir("detect_unsupported");
        fs::write(
            tmp.join("config.json"),
            r#"{"architectures": ["LlamaForCausalLM"]}"#,
        )
        .unwrap();
        assert!(detect_provider(&tmp).is_err());
    }

    // ── Custom model file resolution ────────────────────────────────

    #[test]
    fn custom_model_file_paths_whisper() {
        let tmp = test_dir("custom_whisper");
        fs::write(tmp.join("config.json"), r#"{}"#).unwrap();
        fs::write(tmp.join("tokenizer.json"), r#"{}"#).unwrap();
        fs::write(tmp.join("model.safetensors"), b"weights").unwrap();

        let paths = get_custom_model_file_paths(&tmp).unwrap();
        assert!(
            paths
                .iter()
                .any(|p| p.file_name().unwrap() == "config.json")
        );
        assert!(
            paths
                .iter()
                .any(|p| p.file_name().unwrap() == "tokenizer.json")
        );
        assert!(
            paths
                .iter()
                .any(|p| p.file_name().unwrap() == "model.safetensors")
        );
    }

    #[test]
    fn custom_model_file_paths_voxtral() {
        let tmp = test_dir("custom_voxtral");
        fs::write(tmp.join("config.json"), r#"{}"#).unwrap();
        fs::write(tmp.join("tekken.json"), r#"{}"#).unwrap();
        fs::write(tmp.join("model-00001-of-00002.safetensors"), b"w1").unwrap();
        fs::write(tmp.join("model-00002-of-00002.safetensors"), b"w2").unwrap();

        let paths = get_custom_model_file_paths(&tmp).unwrap();
        assert_eq!(
            paths
                .iter()
                .filter(|p| { p.extension().and_then(|e| e.to_str()) == Some("safetensors") })
                .count(),
            2
        );
    }

    #[test]
    fn custom_model_file_paths_missing_config_fails() {
        let tmp = test_dir("custom_no_config");
        fs::write(tmp.join("model.safetensors"), b"weights").unwrap();
        assert!(get_custom_model_file_paths(&tmp).is_err());
    }

    // ── Custom model discovery ──────────────────────────────────────

    #[test]
    fn discover_finds_valid_models() {
        let tmp = test_dir("discover");

        // A valid whisper model
        let whisper_dir = tmp.join("my-whisper");
        fs::create_dir_all(&whisper_dir).unwrap();
        fs::write(
            whisper_dir.join("config.json"),
            r#"{"architectures": ["WhisperForConditionalGeneration"]}"#,
        )
        .unwrap();

        // A valid voxtral model
        let voxtral_dir = tmp.join("my-voxtral");
        fs::create_dir_all(&voxtral_dir).unwrap();
        fs::write(
            voxtral_dir.join("config.json"),
            r#"{"architectures": ["VoxtralForConditionalGeneration"]}"#,
        )
        .unwrap();

        // Not a model (no config.json at all)
        let junk_dir = tmp.join("not-a-model");
        fs::create_dir_all(&junk_dir).unwrap();
        fs::write(junk_dir.join("readme.txt"), "hi").unwrap();

        // Has config.json but it's not a model config (e.g. ~/.docker)
        let docker_dir = tmp.join(".docker");
        fs::create_dir_all(&docker_dir).unwrap();
        fs::write(docker_dir.join("config.json"), r#"{"auths": {}}"#).unwrap();

        let models = discover_custom_models(&tmp);
        assert_eq!(models.len(), 2);

        let whisper = models.iter().find(|m| m.name == "my-whisper").unwrap();
        assert_eq!(whisper.provider, Provider::LocalWhisper);

        let voxtral = models.iter().find(|m| m.name == "my-voxtral").unwrap();
        assert_eq!(voxtral.provider, Provider::LocalVoxtral);
    }

    #[test]
    fn discover_empty_dir_returns_empty() {
        let tmp = test_dir("discover_empty");
        assert!(discover_custom_models(&tmp).is_empty());
    }

    #[test]
    fn discover_nonexistent_dir_returns_empty() {
        let path = PathBuf::from("/tmp/super-stt-tests/nonexistent-dir");
        assert!(discover_custom_models(&path).is_empty());
    }
}
