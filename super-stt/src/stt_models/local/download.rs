// SPDX-License-Identifier: GPL-3.0-only
use crate::download_progress::DownloadProgressTracker;
use anyhow::Result;
use futures::StreamExt;
use log::{debug, info, warn};
use ring::digest::{Context, SHA256};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use super_stt_shared::stt_model::STTModel;
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

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
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

/// Download model files with progress tracking
///
/// # Errors
///
/// Returns an error if any file download, file system operation,
/// or progress update fails.
pub async fn with_progress(model: &STTModel, tracker: Arc<DownloadProgressTracker>) -> Result<()> {
    // Build the file list based on the specific model
    let mut files = if model.is_voxtral() {
        vec!["config.json", "tekken.json"]
    } else {
        vec!["config.json", "tokenizer.json"]
    };

    // Add model-specific safetensors files
    let safetensors_files = match model {
        STTModel::VoxtralMini => vec![
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
        ],
        STTModel::VoxtralSmall => vec![
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
        _ => vec!["model.safetensors"], // Any whisper model
    };

    files.extend(safetensors_files);

    // Set total file count
    tracker.total_files.store(files.len(), Ordering::Relaxed);

    // Broadcast initial progress
    tracker.broadcast_progress().await;

    // Use the model's own method to get the correct model ID and revision
    let (model_id, revision) = model.model_and_revision();

    // Download each file
    for (index, filename) in files.iter().enumerate() {
        if tracker.is_cancelled() {
            return Err(anyhow::anyhow!("Download was cancelled"));
        }

        cancellable_download(model_id, revision, filename, Arc::clone(&tracker), index).await?;
    }

    // Mark download as complete
    tracker.mark_completed();
    tracker.broadcast_progress().await;

    Ok(())
}

/// Get the expected filenames for a model
fn model_filenames(model: STTModel) -> Vec<&'static str> {
    let mut files = if model.is_voxtral() {
        vec!["config.json", "tekken.json"]
    } else {
        vec!["config.json", "tokenizer.json"]
    };

    let safetensors_files = match model {
        STTModel::VoxtralMini => vec![
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
        ],
        STTModel::VoxtralSmall => vec![
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

/// Check if all model files exist under the given directory.
/// Returns `Some(paths)` if all files are present, `None` otherwise.
fn try_resolve_files(dir: &Path, files: &[&str]) -> Option<Vec<PathBuf>> {
    let mut paths = Vec::with_capacity(files.len());
    for filename in files {
        let path = dir.join(filename);
        if path.exists() {
            paths.push(path);
        } else {
            info!("Missing file in override: {}/{filename}", dir.display());
            return None;
        }
    }
    Some(paths)
}

/// Get the file paths for a model, checking the override path first.
///
/// When `override_path` is set, looks for model files under
/// `{override_path}/{model}/snapshots/main/` (standard `HuggingFace` cache
/// layout without the `models--` prefix).
/// If not found there, falls back to the default `HuggingFace` cache.
///
/// # Errors
///
/// Returns an error if the model files cannot be found in any location.
pub fn get_model_file_paths(model: &STTModel, override_path: Option<&str>) -> Result<Vec<PathBuf>> {
    let files = model_filenames(*model);
    let (model_id, revision) = model.model_and_revision();

    // Check override path first: {override}/{model}/snapshots/{revision}/
    if let Some(dir) = override_path {
        let snapshot_dir = Path::new(dir)
            .join(model.to_string())
            .join("snapshots")
            .join(revision);
        if let Some(paths) = try_resolve_files(&snapshot_dir, &files) {
            info!("Using model override: {}", snapshot_dir.display());
            return Ok(paths);
        }
    }

    // Fall back to default HuggingFace cache
    let mut file_paths = Vec::new();
    for filename in &files {
        let (symlink_path, _blob_path) = get_cache_paths(model_id, revision, filename)?;
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

    fn create_model_files(dir: &Path, model: &STTModel) {
        let (_, revision) = model.model_and_revision();
        let snapshot_dir = dir.join(model.to_string()).join("snapshots").join(revision);
        fs::create_dir_all(&snapshot_dir).unwrap();
        for filename in model_filenames(*model) {
            fs::write(snapshot_dir.join(filename), b"test").unwrap();
        }
    }

    #[test]
    fn override_path_found() {
        let tmp = test_dir("override_found");
        let model = STTModel::WhisperTiny;
        create_model_files(&tmp, &model);

        let paths =
            get_model_file_paths(&model, Some(tmp.to_str().unwrap())).expect("should find files");

        assert_eq!(paths.len(), model_filenames(model).len());
        for path in &paths {
            assert!(path.starts_with(&tmp));
        }
    }

    #[test]
    fn override_path_missing_model_falls_back_to_cache() {
        let tmp = test_dir("override_missing");
        let model = STTModel::WhisperTiny;

        let result = get_model_file_paths(&model, Some(tmp.to_str().unwrap()));
        // Falls through to HF cache — may succeed or fail depending on local cache
        if let Ok(paths) = &result {
            // If it succeeded, files should NOT be in the override dir
            for path in paths {
                assert!(!path.starts_with(&tmp));
            }
        }
    }

    #[test]
    fn override_partial_files_falls_back_to_cache() {
        let tmp = test_dir("override_partial");
        let model = STTModel::WhisperTiny;
        let (_, revision) = model.model_and_revision();
        let snapshot_dir = tmp.join(model.to_string()).join("snapshots").join(revision);
        fs::create_dir_all(&snapshot_dir).unwrap();
        fs::write(snapshot_dir.join("config.json"), b"test").unwrap();

        let result = get_model_file_paths(&model, Some(tmp.to_str().unwrap()));
        // Partial override — falls through to HF cache
        if let Ok(paths) = &result {
            for path in paths {
                assert!(!path.starts_with(&tmp));
            }
        }
    }

    #[test]
    fn override_uses_model_display_name() {
        let tmp = test_dir("override_display_name");
        let model = STTModel::WhisperTinyEn;
        create_model_files(&tmp, &model);

        let paths =
            get_model_file_paths(&model, Some(tmp.to_str().unwrap())).expect("should find files");

        let first = paths[0].to_str().unwrap();
        assert!(first.contains("whisper-tiny.en"));
    }
}
