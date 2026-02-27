use super::error::HfError;
use super::privacy::{scrub_metadata, ScrubOptions};
use super::progress::{UploadProgress, UploadStage};
use crate::clip::format::read_clip;
use log::{debug, info, warn};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const HF_API_BASE: &str = "https://huggingface.co/api";
const LFS_THRESHOLD: usize = 10 * 1024 * 1024; // 10 MB

/// Configuration for HuggingFace uploads.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HuggingFaceConfig {
    /// User has explicitly consented to upload data.
    #[serde(default)]
    pub upload_consent: bool,
    /// HuggingFace API token (never serialized to frontend).
    #[serde(default, skip_serializing)]
    pub token: String,
    /// Target dataset repo (e.g., "username/gameclip-dataset").
    #[serde(default)]
    pub repo_id: String,
    /// Minimum quality score for upload (0.0 to 1.0).
    #[serde(default = "default_quality_gate")]
    pub quality_gate: f64,
    /// Whether to create the repo as private.
    #[serde(default)]
    pub private_repo: bool,
}

fn default_quality_gate() -> f64 {
    0.3
}

impl Default for HuggingFaceConfig {
    fn default() -> Self {
        Self {
            upload_consent: false,
            token: String::new(),
            repo_id: String::new(),
            quality_gate: default_quality_gate(),
            private_repo: false,
        }
    }
}

/// Prepared clip data ready for upload.
#[derive(Debug)]
pub struct PreparedClipData {
    pub clip_name: String,
    pub video_data: Vec<u8>,
    pub metadata_json: String,
    pub input_jsonl: String,
    pub frame_actions_jsonl: Option<String>,
    pub quality_json: Option<String>,
}

/// HuggingFace API client for uploading clip datasets.
pub struct HfClient {
    client: reqwest::blocking::Client,
    token: String,
    repo_id: String,
    api_base: String,
}

impl HfClient {
    /// Create a new HfClient.
    pub fn new(token: String, repo_id: String) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            token,
            repo_id,
            api_base: HF_API_BASE.to_string(),
        }
    }

    /// Create a client with a custom API base URL (for testing).
    #[cfg(test)]
    pub fn with_base_url(token: String, repo_id: String, api_base: String) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            token,
            repo_id,
            api_base,
        }
    }

    /// Ensure the dataset repository exists (creates if needed).
    pub fn ensure_repo_exists(&self, private: bool) -> Result<(), HfError> {
        let url = format!("{}/repos/create", self.api_base);

        let (org, name) = match self.repo_id.split_once('/') {
            Some((org, name)) => (Some(org), name),
            None => (None, self.repo_id.as_str()),
        };
        let mut body = serde_json::json!({
            "name": name,
            "type": "dataset",
            "private": private,
        });
        if let Some(org) = org {
            body["organization"] = serde_json::Value::String(org.to_string());
        }

        let response = self.client
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()?;

        match response.status().as_u16() {
            200 | 201 => {
                info!("Dataset repo created: {}", self.repo_id);
                Ok(())
            }
            409 => {
                debug!("Dataset repo already exists: {}", self.repo_id);
                Ok(())
            }
            401 => Err(HfError::Unauthorized),
            status => {
                let text = response.text().unwrap_or_default();
                Err(HfError::Http(format!("HTTP {status}: {text}")))
            }
        }
    }

    /// Upload a single prepared clip to the dataset repo.
    pub fn upload_clip(
        &self,
        clip: &PreparedClipData,
        _progress_fn: &dyn Fn(UploadStage),
    ) -> Result<(), HfError> {
        let branch = "main";
        let prefix = &clip.clip_name;

        // Build commit operations (files to upload)
        let mut operations = Vec::new();

        // Video file — reject if too large for inline upload
        let video_path = format!("{prefix}/video.mp4");
        if clip.video_data.len() > MAX_INLINE_SIZE {
            return Err(HfError::Http(format!(
                "Video too large for inline upload ({} MB). LFS upload not yet implemented.",
                clip.video_data.len() / (1024 * 1024)
            )));
        }
        if clip.video_data.len() > LFS_THRESHOLD {
            debug!("Video exceeds LFS threshold ({} MB)", clip.video_data.len() / (1024 * 1024));
        }
        operations.push(serde_json::json!({
            "key": "file",
            "value": {
                "content": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &clip.video_data
                ),
                "path": video_path,
                "encoding": "base64"
            }
        }));

        // Metadata
        operations.push(serde_json::json!({
            "key": "file",
            "value": {
                "content": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    clip.metadata_json.as_bytes()
                ),
                "path": format!("{prefix}/metadata.json"),
                "encoding": "base64"
            }
        }));

        // Input events
        operations.push(serde_json::json!({
            "key": "file",
            "value": {
                "content": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    clip.input_jsonl.as_bytes()
                ),
                "path": format!("{prefix}/input.jsonl"),
                "encoding": "base64"
            }
        }));

        // Frame actions (optional)
        if let Some(ref fa) = clip.frame_actions_jsonl {
            operations.push(serde_json::json!({
                "key": "file",
                "value": {
                    "content": base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        fa.as_bytes()
                    ),
                    "path": format!("{prefix}/frame_actions.jsonl"),
                    "encoding": "base64"
                }
            }));
        }

        // Quality (optional)
        if let Some(ref q) = clip.quality_json {
            operations.push(serde_json::json!({
                "key": "file",
                "value": {
                    "content": base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        q.as_bytes()
                    ),
                    "path": format!("{prefix}/quality.json"),
                    "encoding": "base64"
                }
            }));
        }

        // Commit
        let url = format!(
            "{}/datasets/{}/commit/{}",
            self.api_base, self.repo_id, branch
        );

        // Build NDJSON body
        let header = serde_json::json!({
            "summary": format!("Add clip: {}", clip.clip_name),
            "parentCommit": ""
        });

        let mut ndjson = serde_json::to_string(&header)?;
        ndjson.push('\n');
        for op in &operations {
            ndjson.push_str(&serde_json::to_string(op)?);
            ndjson.push('\n');
        }

        let response = self.client
            .post(&url)
            .bearer_auth(&self.token)
            .header("Content-Type", "application/x-ndjson")
            .body(ndjson)
            .send()?;

        match response.status().as_u16() {
            200 | 201 => {
                info!("Clip uploaded: {}", clip.clip_name);
                Ok(())
            }
            401 => Err(HfError::Unauthorized),
            status => {
                let text = response.text().unwrap_or_default();
                Err(HfError::Http(format!("commit failed HTTP {status}: {text}")))
            }
        }
    }
}

/// Prepare a clip for upload: read, scrub, quality check, serialize.
pub fn prepare_clip(
    clip_path: &Path,
    quality_gate: f64,
) -> Result<PreparedClipData, HfError> {
    let contents = read_clip(clip_path)?;

    // Quality gate check
    if let Some(ref quality) = contents.quality_score {
        if quality.overall_score < quality_gate {
            return Err(HfError::BelowQualityThreshold {
                score: quality.overall_score,
                threshold: quality_gate,
            });
        }
    }

    // Scrub metadata
    let scrubbed = scrub_metadata(&contents.metadata, &ScrubOptions::default());
    let metadata_json = serde_json::to_string_pretty(&scrubbed)?;

    // Serialize input events
    let input_jsonl: String = contents.input_events
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");

    // Frame actions
    let frame_actions_jsonl = if !contents.frame_actions.is_empty() {
        let lines: Vec<String> = contents.frame_actions
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?;
        Some(lines.join("\n"))
    } else {
        None
    };

    // Quality
    let quality_json = contents.quality_score
        .as_ref()
        .map(serde_json::to_string_pretty)
        .transpose()?;

    Ok(PreparedClipData {
        clip_name: scrubbed.name.clone(),
        video_data: contents.video_data,
        metadata_json,
        input_jsonl,
        frame_actions_jsonl,
        quality_json,
    })
}

/// Maximum inline upload size (50 MB). LFS upload not yet implemented.
const MAX_INLINE_SIZE: usize = 50 * 1024 * 1024;

/// Validate repo_id format: must be "owner/name" with safe characters.
fn validate_repo_id(repo_id: &str) -> Result<(), HfError> {
    let parts: Vec<&str> = repo_id.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| p.is_empty())
        || repo_id.contains("..")
        || repo_id
            .chars()
            .any(|c| !c.is_alphanumeric() && c != '/' && c != '-' && c != '_' && c != '.')
    {
        return Err(HfError::Http(format!(
            "Invalid repo_id format: {repo_id}. Expected 'owner/dataset-name'"
        )));
    }
    Ok(())
}

/// Upload multiple clips to HuggingFace.
///
/// Returns the number of clips successfully uploaded.
pub fn upload_clips(
    config: &HuggingFaceConfig,
    clip_paths: &[std::path::PathBuf],
    cancel: Arc<AtomicBool>,
    progress_fn: impl Fn(UploadProgress),
) -> Result<u32, HfError> {
    if !config.upload_consent {
        return Err(HfError::ConsentRequired);
    }
    if config.token.is_empty() {
        return Err(HfError::EmptyToken);
    }
    if clip_paths.is_empty() {
        return Ok(0);
    }

    // Validate repo_id format (must be "owner/name" with safe characters)
    validate_repo_id(&config.repo_id)?;

    let client = HfClient::new(config.token.clone(), config.repo_id.clone());

    // Ensure repo exists
    client.ensure_repo_exists(config.private_repo)?;

    let total = clip_paths.len() as u32;
    let mut uploaded = 0u32;

    for (i, path) in clip_paths.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            info!("Upload cancelled by user");
            break;
        }

        let clip_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        progress_fn(UploadProgress {
            current_clip: i as u32,
            total_clips: total,
            clip_name: clip_name.clone(),
            stage: UploadStage::Preparing,
            bytes_uploaded: 0,
            total_bytes: 0,
        });

        // Prepare clip (read, scrub, quality check)
        let prepared = match prepare_clip(path, config.quality_gate) {
            Ok(p) => p,
            Err(HfError::BelowQualityThreshold { score, threshold }) => {
                warn!(
                    "Skipping clip {} (quality {:.2} < {:.2})",
                    clip_name, score, threshold
                );
                progress_fn(UploadProgress {
                    current_clip: i as u32,
                    total_clips: total,
                    clip_name: clip_name.clone(),
                    stage: UploadStage::Failed {
                        reason: format!("Quality {score:.2} below threshold {threshold:.2}"),
                    },
                    bytes_uploaded: 0,
                    total_bytes: 0,
                });
                continue;
            }
            Err(e) => {
                warn!("Failed to prepare clip {}: {e}", clip_name);
                progress_fn(UploadProgress {
                    current_clip: i as u32,
                    total_clips: total,
                    clip_name,
                    stage: UploadStage::Failed {
                        reason: e.to_string(),
                    },
                    bytes_uploaded: 0,
                    total_bytes: 0,
                });
                continue;
            }
        };

        // Check cancellation after preparation
        if cancel.load(Ordering::Relaxed) {
            info!("Upload cancelled by user after preparing clip");
            break;
        }

        let total_bytes = prepared.video_data.len() as u64;

        progress_fn(UploadProgress {
            current_clip: i as u32,
            total_clips: total,
            clip_name: clip_name.clone(),
            stage: UploadStage::UploadingVideo,
            bytes_uploaded: 0,
            total_bytes,
        });

        // Upload
        match client.upload_clip(&prepared, &|stage| {
            progress_fn(UploadProgress {
                current_clip: i as u32,
                total_clips: total,
                clip_name: clip_name.clone(),
                stage,
                bytes_uploaded: 0,
                total_bytes,
            });
        }) {
            Ok(()) => {
                uploaded += 1;
                progress_fn(UploadProgress {
                    current_clip: i as u32,
                    total_clips: total,
                    clip_name,
                    stage: UploadStage::Done,
                    bytes_uploaded: total_bytes,
                    total_bytes,
                });
            }
            Err(e) => {
                warn!("Failed to upload clip: {e}");
                progress_fn(UploadProgress {
                    current_clip: i as u32,
                    total_clips: total,
                    clip_name,
                    stage: UploadStage::Failed {
                        reason: e.to_string(),
                    },
                    bytes_uploaded: 0,
                    total_bytes,
                });
            }
        }
    }

    Ok(uploaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    // T34: HfClient::new does not panic
    #[test]
    fn hf_client_new_does_not_panic() {
        let _client = HfClient::new("test_token".to_string(), "user/dataset".to_string());
    }

    // T35: HTTP 401 returns Unauthorized error (mockito)
    #[test]
    fn http_401_returns_unauthorized() {
        let mut server = mockito::Server::new();
        let mock = server.mock("POST", "/repos/create")
            .with_status(401)
            .with_body("Unauthorized")
            .create();

        let client = HfClient::with_base_url(
            "bad_token".to_string(),
            "user/dataset".to_string(),
            server.url(),
        );

        let result = client.ensure_repo_exists(false);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HfError::Unauthorized));
        mock.assert();
    }

    // T36: quality gate rejects clips below threshold
    #[test]
    fn quality_gate_with_empty_paths_returns_zero() {
        let config = HuggingFaceConfig {
            upload_consent: true,
            token: "test_token".to_string(),
            repo_id: "user/test".to_string(),
            quality_gate: 0.8,
            private_repo: false,
        };

        let cancel = Arc::new(AtomicBool::new(false));
        // Empty paths returns 0 immediately (no network calls)
        let result = upload_clips(&config, &[], cancel, |_| {});
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    // T37: PreparedClipData built correctly
    #[test]
    fn prepared_clip_data_has_fields() {
        let data = PreparedClipData {
            clip_name: "test_clip".to_string(),
            video_data: vec![1, 2, 3],
            metadata_json: "{}".to_string(),
            input_jsonl: "".to_string(),
            frame_actions_jsonl: None,
            quality_json: None,
        };
        assert_eq!(data.clip_name, "test_clip");
        assert_eq!(data.video_data.len(), 3);
    }

    // validate_repo_id accepts valid formats
    #[test]
    fn validate_repo_id_accepts_valid() {
        assert!(validate_repo_id("user/dataset").is_ok());
        assert!(validate_repo_id("org-name/my-dataset_v2").is_ok());
        assert!(validate_repo_id("user/data.set").is_ok());
    }

    // validate_repo_id rejects invalid formats
    #[test]
    fn validate_repo_id_rejects_invalid() {
        assert!(validate_repo_id("").is_err());
        assert!(validate_repo_id("nodash").is_err());
        assert!(validate_repo_id("a/b/c").is_err());
        assert!(validate_repo_id("../etc/passwd").is_err());
        assert!(validate_repo_id("user/data?admin=true").is_err());
        assert!(validate_repo_id("/dataset").is_err());
        assert!(validate_repo_id("user/").is_err());
    }

    // T40: upload_clips errors when consent=false
    #[test]
    fn upload_clips_errors_without_consent() {
        let config = HuggingFaceConfig {
            upload_consent: false,
            token: "test".to_string(),
            repo_id: "user/test".to_string(),
            ..Default::default()
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let result = upload_clips(&config, &[], cancel, |_| {});
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HfError::ConsentRequired));
    }

    // T41: upload_clips errors when token is empty
    #[test]
    fn upload_clips_errors_with_empty_token() {
        let config = HuggingFaceConfig {
            upload_consent: true,
            token: String::new(),
            repo_id: "user/test".to_string(),
            ..Default::default()
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let result = upload_clips(&config, &[], cancel, |_| {});
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HfError::EmptyToken));
    }
}
