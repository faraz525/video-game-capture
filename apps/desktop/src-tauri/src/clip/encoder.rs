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
    /// Uses NVENC if available, otherwise falls back to libx264.
    pub fn start(
        output_path: &Path,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self, EncoderError> {
        let ffmpeg_path = find_ffmpeg()?;

        // Try NVENC first
        match Self::start_with_codec(
            &ffmpeg_path,
            output_path,
            width,
            height,
            fps,
            "h264_nvenc",
            &["-preset", "p4", "-rc", "vbr", "-cq", "23"],
        ) {
            Ok(encoder) => return Ok(encoder),
            Err(e) => {
                eprintln!("[GameClip] NVENC unavailable ({e}), falling back to libx264");
            }
        }

        // Fallback to libx264
        Self::start_with_codec(
            &ffmpeg_path,
            output_path,
            width,
            height,
            fps,
            "libx264",
            &["-preset", "ultrafast", "-crf", "23"],
        )
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

        stdin.write_all(rgba_bytes)?;
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

/// Find the FFmpeg executable.
///
/// Checks the Tauri sidecar path first, then falls back to system PATH.
fn find_ffmpeg() -> Result<String, EncoderError> {
    // Check if bundled as Tauri sidecar
    if let Ok(exe_dir) = std::env::current_exe().map(|p| p.parent().unwrap_or(Path::new(".")).to_path_buf()) {
        let sidecar = exe_dir.join("ffmpeg").with_extension(std::env::consts::EXE_EXTENSION);
        if sidecar.exists() {
            return Ok(sidecar.to_string_lossy().to_string());
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
            "ffmpeg not found in sidecar path or system PATH".to_string(),
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

    let temp_dir = tempfile::TempDir::new()?;
    let output_path = temp_dir.path().join("clip.mp4");

    let mut encoder = FfmpegEncoder::start(&output_path, width, height, fps)?;

    for frame in frames {
        encoder.write_frame(&frame.data)?;
    }

    let stderr = encoder.finish()?;
    eprintln!("[GameClip] FFmpeg encode complete: {stderr}");

    let mp4_data = std::fs::read(&output_path)?;
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
