use log::{debug, info, warn};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Error type for FFmpeg encoding operations.
#[derive(Debug, thiserror::Error)]
pub enum EncoderError {
    #[error("FFmpeg not found: {0}")]
    NotFound(String),
    #[error("FFmpeg process error: {0}")]
    Process(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encoding failed: {0}")]
    EncodingFailed(String),
}

/// FFmpeg subprocess encoder that pipes raw frames via stdin.
///
/// Attempts NVENC (h264_nvenc) first for GPU-accelerated encoding,
/// falls back to libx264 if NVENC is unavailable.
pub struct FfmpegEncoder {
    child: Child,
}

impl FfmpegEncoder {
    /// Start an FFmpeg encoding process.
    ///
    /// The encoder accepts raw RGBA frames via stdin and produces an MP4 file.
    /// Codec priority: VideoToolbox (macOS) → NVENC (Windows) → libx264 (fallback).
    pub fn start(
        output_path: &Path,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self, EncoderError> {
        let ffmpeg_path = find_ffmpeg()?;
        info!("FFmpeg found at: {ffmpeg_path}");

        // Codec priority order based on platform
        let codecs: &[(&str, &[&str])] = &[
            #[cfg(target_os = "macos")]
            ("h264_videotoolbox", &["-q:v", "65", "-allow_sw", "1"]),
            #[cfg(target_os = "windows")]
            ("h264_nvenc", &["-preset", "p4", "-rc", "vbr", "-cq", "23"]),
            ("libx264", &["-preset", "ultrafast", "-crf", "23"]),
        ];

        let mut last_err = None;
        for (codec, codec_args) in codecs {
            if !is_codec_available(&ffmpeg_path, codec) {
                info!("{codec} not available, skipping");
                continue;
            }
            info!("Trying encoder: {codec}");
            match Self::start_with_codec(
                &ffmpeg_path,
                output_path,
                width,
                height,
                fps,
                codec,
                codec_args,
            ) {
                Ok(encoder) => return Ok(encoder),
                Err(e) => {
                    warn!("{codec} failed to start: {e}");
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            EncoderError::NotFound("no working H.264 encoder found".to_string())
        }))
    }

    fn start_with_codec(
        ffmpeg_path: &str,
        output_path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        codec: &str,
        codec_args: &[&str],
    ) -> Result<Self, EncoderError> {
        let mut cmd = Command::new(ffmpeg_path);
        cmd.args([
            "-y",
            "-f", "rawvideo",
            "-pix_fmt", "rgba",
            "-s", &format!("{width}x{height}"),
            "-r", &fps.to_string(),
            "-i", "pipe:0",
            "-c:v", codec,
        ])
        .args(codec_args)
        .args([
            "-pix_fmt", "yuv420p",
            "-movflags", "+faststart",
        ])
        .arg(output_path.as_os_str())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| EncoderError::Process(format!("Failed to start FFmpeg ({codec}): {e}")))?;

        Ok(Self { child })
    }

    /// Write a raw RGBA frame to the encoder.
    pub fn write_frame(&mut self, rgba_bytes: &[u8]) -> Result<(), EncoderError> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| EncoderError::Process("stdin closed".to_string()))?;

        if let Err(e) = stdin.write_all(rgba_bytes) {
            // On broken pipe, capture FFmpeg's stderr to diagnose the failure
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                drop(self.child.stdin.take());
                // Read whatever stderr is available without waiting
                if let Some(stderr_pipe) = self.child.stderr.as_mut() {
                    use std::io::Read;
                    let mut stderr_buf = Vec::new();
                    let _ = stderr_pipe.read_to_end(&mut stderr_buf);
                    let stderr = String::from_utf8_lossy(&stderr_buf);
                    let last_line = stderr.lines().last().unwrap_or("no output");
                    warn!("FFmpeg stderr on broken pipe: {last_line}");
                    return Err(EncoderError::EncodingFailed(format!(
                        "FFmpeg pipe broken: {last_line}"
                    )));
                }
            }
            return Err(EncoderError::Io(e));
        }
        Ok(())
    }

    /// Finish encoding and wait for FFmpeg to complete.
    ///
    /// Returns the FFmpeg stderr output for logging.
    pub fn finish(mut self) -> Result<String, EncoderError> {
        // Close stdin to signal end of input
        drop(self.child.stdin.take());

        let output = self
            .child
            .wait_with_output()
            .map_err(|e| EncoderError::Process(format!("Failed to wait for FFmpeg: {e}")))?;

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            return Err(EncoderError::EncodingFailed(format!(
                "FFmpeg exited with status {}: {}",
                output.status, stderr
            )));
        }

        Ok(stderr)
    }
}

/// Check if a given codec is available in FFmpeg's encoder list.
fn is_codec_available(ffmpeg_path: &str, codec: &str) -> bool {
    let output = Command::new(ffmpeg_path)
        .args(["-hide_banner", "-encoders"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).contains(codec),
        Err(_) => false,
    }
}

/// Find the FFmpeg executable.
///
/// Search order: Tauri sidecar → well-known paths → system PATH.
fn find_ffmpeg() -> Result<String, EncoderError> {
    // Check if bundled as Tauri sidecar
    if let Ok(exe_dir) = std::env::current_exe().map(|p| p.parent().unwrap_or(Path::new(".")).to_path_buf()) {
        let sidecar = exe_dir.join("ffmpeg").with_extension(std::env::consts::EXE_EXTENSION);
        if sidecar.exists() {
            return Ok(sidecar.to_string_lossy().to_string());
        }
    }

    // Check well-known paths (macOS GUI apps may not inherit full shell PATH)
    let well_known = [
        "/opt/homebrew/bin/ffmpeg",   // Apple Silicon homebrew
        "/usr/local/bin/ffmpeg",      // Intel homebrew / manual install
    ];
    for path in &well_known {
        if Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }

    // Fall back to system PATH
    let test = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match test {
        Ok(status) if status.success() => Ok("ffmpeg".to_string()),
        _ => Err(EncoderError::NotFound(
            "ffmpeg not found in sidecar, /opt/homebrew/bin, /usr/local/bin, or system PATH".to_string(),
        )),
    }
}

/// Encode raw RGBA frames into an MP4 file using FFmpeg.
///
/// Pipes RGBA frame data directly to FFmpeg (which is configured with
/// `-pix_fmt rgba` input). No pixel format conversion needed.
pub fn encode_frames_to_mp4(
    frames: &[crate::capture::CapturedFrame],
    fps: u32,
) -> Result<Vec<u8>, EncoderError> {
    if frames.is_empty() {
        return Err(EncoderError::EncodingFailed("no frames to encode".to_string()));
    }

    let width = frames[0].width;
    let height = frames[0].height;
    let frame_count = frames.len();

    info!("Encoding {frame_count} frames ({width}x{height} @ {fps}fps)");

    let temp_dir = tempfile::TempDir::new()?;
    let output_path = temp_dir.path().join("clip.mp4");

    let mut encoder = FfmpegEncoder::start(&output_path, width, height, fps)?;

    for (i, frame) in frames.iter().enumerate() {
        if let Err(e) = encoder.write_frame(&frame.data) {
            warn!("FFmpeg write failed at frame {i}/{frame_count}: {e}");
            return Err(e);
        }
    }

    let stderr = encoder.finish()?;
    debug!("FFmpeg encode complete: {stderr}");

    let mp4_data = std::fs::read(&output_path)?;
    info!("Encoded {frame_count} frames → {} bytes MP4", mp4_data.len());
    Ok(mp4_data)
}

/// Re-encode raw RGBA frame data into an MP4 file.
///
/// Used to convert clips that were saved with the raw RGBA fallback
/// into playable MP4 video. The raw data is expected to be a sequence
/// of RGBA frames of the given dimensions.
pub fn reencode_raw_to_mp4(
    raw_data: &[u8],
    width: u32,
    height: u32,
    fps: u32,
) -> Result<Vec<u8>, EncoderError> {
    let frame_size = (width * height * 4) as usize;
    if raw_data.is_empty() || frame_size == 0 {
        return Err(EncoderError::EncodingFailed("no data to re-encode".to_string()));
    }

    let frame_count = raw_data.len() / frame_size;
    if frame_count == 0 {
        return Err(EncoderError::EncodingFailed(format!(
            "raw data size {} is smaller than one frame ({} bytes)",
            raw_data.len(),
            frame_size
        )));
    }

    info!("Re-encoding {frame_count} raw frames ({width}x{height} @ {fps}fps)");

    let temp_dir = tempfile::TempDir::new()?;
    let output_path = temp_dir.path().join("clip.mp4");

    let mut encoder = FfmpegEncoder::start(&output_path, width, height, fps)?;

    for i in 0..frame_count {
        let start = i * frame_size;
        let end = start + frame_size;
        encoder.write_frame(&raw_data[start..end])?;
    }

    let stderr = encoder.finish()?;
    debug!("Re-encode complete: {stderr}");

    let mp4_data = std::fs::read(&output_path)?;
    info!("Re-encoded {frame_count} frames → {} bytes MP4", mp4_data.len());
    Ok(mp4_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_ffmpeg_does_not_panic() {
        // This test just verifies the function doesn't panic.
        // It may or may not find ffmpeg depending on the system.
        let _ = find_ffmpeg();
    }
}
