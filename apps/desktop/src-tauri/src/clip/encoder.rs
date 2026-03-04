use crate::capture::FramePixelFormat;
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
    /// The encoder accepts raw frames via stdin and produces an MP4 file.
    /// `pixel_format` determines the FFmpeg `-pix_fmt` input flag.
    /// Codec priority: VideoToolbox (macOS) → NVENC (Windows) → libx264 (fallback).
    pub fn start(
        output_path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        pixel_format: FramePixelFormat,
    ) -> Result<Self, EncoderError> {
        let ffmpeg_path = find_ffmpeg()?;
        info!("FFmpeg found at: {ffmpeg_path}");

        // Codec priority order based on platform.
        // Hardware encoders use baseline profile (no B-frames) for lower latency.
        let codecs: &[(&str, &[&str])] = &[
            #[cfg(target_os = "macos")]
            ("h264_videotoolbox", &["-allow_sw", "1", "-prio_speed", "1", "-profile:v", "baseline"]),
            #[cfg(target_os = "windows")]
            ("h264_nvenc", &["-preset", "p4", "-tune", "ll", "-profile:v", "baseline", "-rc", "vbr", "-cq", "23"]),
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
                pixel_format,
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

    #[allow(clippy::too_many_arguments)]
    fn start_with_codec(
        ffmpeg_path: &str,
        output_path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        codec: &str,
        codec_args: &[&str],
        pixel_format: FramePixelFormat,
    ) -> Result<Self, EncoderError> {
        let input_pix_fmt = match pixel_format {
            FramePixelFormat::Bgra => "bgra",
            FramePixelFormat::Rgba => "rgba",
            FramePixelFormat::Nv12 => "nv12",
        };
        let mut cmd = Command::new(ffmpeg_path);
        cmd.args([
            "-y",
            "-f", "rawvideo",
            "-pix_fmt", input_pix_fmt,
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
pub(crate) fn is_codec_available(ffmpeg_path: &str, codec: &str) -> bool {
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

/// Path set by Tauri sidecar resolution at startup.
static SIDECAR_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Set the FFmpeg sidecar path (called from lib.rs at app startup).
#[allow(dead_code)]
pub fn set_sidecar_path(path: String) {
    let _ = SIDECAR_PATH.set(path);
}

/// Find the FFmpeg executable.
///
/// Search order: GAMECLIP_FFMPEG_PATH env → sidecar OnceLock →
/// well-known paths → system PATH.
pub(crate) fn find_ffmpeg() -> Result<String, EncoderError> {
    // 1. Environment variable override
    if let Ok(env_path) = std::env::var("GAMECLIP_FFMPEG_PATH") {
        if Path::new(&env_path).exists() {
            return Ok(env_path);
        }
        return Err(EncoderError::NotFound(format!(
            "GAMECLIP_FFMPEG_PATH set to '{env_path}' but file does not exist"
        )));
    }

    // 2. Sidecar path set at startup
    if let Some(sidecar) = SIDECAR_PATH.get() {
        if Path::new(sidecar).exists() {
            return Ok(sidecar.clone());
        }
    }

    // 3. Check if bundled next to executable
    if let Ok(exe_dir) = std::env::current_exe().map(|p| p.parent().unwrap_or(Path::new(".")).to_path_buf()) {
        // Tauri externalBin naming: ffmpeg-<target-triple>[.exe]
        if let Some(triple) = option_env!("TAURI_ENV_TARGET_TRIPLE") {
            let sidecar_name = if triple.contains("windows") {
                format!("ffmpeg-{triple}.exe")
            } else {
                format!("ffmpeg-{triple}")
            };
            let sidecar = exe_dir.join(sidecar_name);
            if sidecar.exists() {
                return Ok(sidecar.to_string_lossy().to_string());
            }
        }

        // Generic fallback next to executable
        let fallback = exe_dir
            .join("ffmpeg")
            .with_extension(std::env::consts::EXE_EXTENSION);
        if fallback.exists() {
            return Ok(fallback.to_string_lossy().to_string());
        }
    }

    // 4. Check well-known paths (macOS GUI apps may not inherit full shell PATH)
    let well_known = [
        "/opt/homebrew/bin/ffmpeg",   // Apple Silicon homebrew
        "/usr/local/bin/ffmpeg",      // Intel homebrew / manual install
    ];
    for path in &well_known {
        if Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }

    // 5. Fall back to system PATH
    let test = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match test {
        Ok(status) if status.success() => Ok("ffmpeg".to_string()),
        _ => Err(EncoderError::NotFound(
            "ffmpeg not found via env, sidecar, /opt/homebrew/bin, /usr/local/bin, or system PATH".to_string(),
        )),
    }
}

/// Encode raw frames into an MP4 file using FFmpeg.
///
/// Reads the pixel format from the first frame and configures FFmpeg's
/// `-pix_fmt` input flag accordingly (rgba or bgra).
pub fn encode_frames_to_mp4(
    frames: &[crate::capture::CapturedFrame],
    fps: u32,
) -> Result<Vec<u8>, EncoderError> {
    if frames.is_empty() {
        return Err(EncoderError::EncodingFailed("no frames to encode".to_string()));
    }

    let width = frames[0].width;
    let height = frames[0].height;
    let pixel_format = frames[0].pixel_format;
    let frame_count = frames.len();

    info!("Encoding {frame_count} frames ({width}x{height} @ {fps}fps, {pixel_format:?})");

    let temp_dir = tempfile::TempDir::new()?;
    let output_path = temp_dir.path().join("clip.mp4");

    let mut encoder = FfmpegEncoder::start(&output_path, width, height, fps, pixel_format)?;

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

/// Re-encode raw frame data into an MP4 file.
///
/// Used to convert clips that were saved with the raw fallback
/// into playable MP4 video. The raw data is expected to be a sequence
/// of frames of the given dimensions in the specified pixel format.
pub fn reencode_raw_to_mp4(
    raw_data: &[u8],
    width: u32,
    height: u32,
    fps: u32,
) -> Result<Vec<u8>, EncoderError> {
    // Legacy raw clips are always RGBA (the old code converted to RGBA before saving)
    let pixel_format = FramePixelFormat::Rgba;
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

    let mut encoder = FfmpegEncoder::start(&output_path, width, height, fps, pixel_format)?;

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

    // T14: find_ffmpeg respects GAMECLIP_FFMPEG_PATH env override
    #[test]
    fn find_ffmpeg_respects_env_override() {
        let original = std::env::var("GAMECLIP_FFMPEG_PATH").ok();

        // Point to a known existing path (the real ffmpeg)
        if let Ok(real_path) = find_ffmpeg() {
            std::env::set_var("GAMECLIP_FFMPEG_PATH", &real_path);
            let result = find_ffmpeg();
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), real_path);
        }

        // Restore
        match original {
            Some(val) => std::env::set_var("GAMECLIP_FFMPEG_PATH", val),
            None => std::env::remove_var("GAMECLIP_FFMPEG_PATH"),
        }
    }

    // T15: set_sidecar_path makes find_ffmpeg return that path
    #[test]
    fn set_sidecar_path_used_by_find_ffmpeg() {
        // OnceLock can only be set once per process, so this test
        // verifies the API doesn't panic. The actual path resolution
        // depends on whether the OnceLock was already set.
        set_sidecar_path("/some/fake/path".to_string());
        // If GAMECLIP_FFMPEG_PATH is not set, find_ffmpeg will check
        // the sidecar path next. It won't exist, so it falls through.
        let _ = find_ffmpeg();
    }

    // T16: without sidecar or env, falls back to system FFmpeg
    #[test]
    fn find_ffmpeg_fallback_to_system() {
        let original = std::env::var("GAMECLIP_FFMPEG_PATH").ok();
        std::env::remove_var("GAMECLIP_FFMPEG_PATH");

        // Should either find system ffmpeg or return NotFound
        let result = find_ffmpeg();
        // We can't assert success because the test machine might not have ffmpeg
        // but we can assert it doesn't panic
        match &result {
            Ok(path) => assert!(!path.is_empty()),
            Err(e) => assert!(matches!(e, EncoderError::NotFound(_))),
        }

        // Restore
        match original {
            Some(val) => std::env::set_var("GAMECLIP_FFMPEG_PATH", val),
            None => {} // already removed
        }
    }
}
