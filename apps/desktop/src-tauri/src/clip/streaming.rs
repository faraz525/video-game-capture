use crate::capture::{CapturedFrame, FramePixelFormat};
use crate::sync::encoded_ring_buffer::{ChunkType, EncodedChunk};
use log::{debug, info, warn};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// GOP size as a multiple of the frame rate (e.g., 2 means a keyframe every 2 seconds).
/// Shared with engine.rs for computing `fragment_duration_us`.
pub const GOP_MULTIPLIER: u32 = 2;

/// Error type for streaming encoder operations.
#[derive(Debug, thiserror::Error)]
pub enum StreamingEncoderError {
    #[error("FFmpeg not found: {0}")]
    NotFound(String),
    #[error("encoder not running")]
    NotRunning,
    #[error("encoder already running")]
    AlreadyRunning,
    #[error("process error: {0}")]
    Process(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Configuration for the streaming encoder.
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Input pixel format. Determines FFmpeg's `-pix_fmt` input flag.
    pub pixel_format: FramePixelFormat,
}

/// Trait for streaming video encoders that accept raw frames and produce
/// encoded chunks in real-time.
pub trait StreamingEncoder: Send {
    fn start(&mut self, config: StreamingConfig) -> Result<(), StreamingEncoderError>;
    fn push_frame(&mut self, frame: &CapturedFrame) -> Result<(), StreamingEncoderError>;
    fn poll_chunk(&mut self) -> Result<Option<EncodedChunk>, StreamingEncoderError>;
    fn stop(&mut self) -> Result<(), StreamingEncoderError>;
    #[allow(dead_code)]
    fn is_running(&self) -> bool;
    /// Returns the SyncClock timestamp (us) of the first frame pushed.
    fn first_frame_timestamp_us(&self) -> Option<u64>;
    /// Returns the number of frames dropped due to backpressure.
    fn dropped_frame_count(&self) -> u64;
}

/// FFmpeg-based streaming encoder that produces fragmented MP4 chunks.
///
/// Runs FFmpeg as a subprocess with raw RGBA input on stdin and fragmented
/// MP4 output on stdout. A background reader thread parses the stdout
/// stream into MP4 box-level chunks and sends them via an mpsc channel.
pub struct FfmpegStreamingEncoder {
    child: Option<Child>,
    chunk_rx: Option<mpsc::Receiver<EncodedChunk>>,
    reader_handle: Option<thread::JoinHandle<()>>,
    /// Dedicated writer thread that performs blocking stdin writes off the capture loop.
    writer_handle: Option<thread::JoinHandle<()>>,
    /// Bounded channel to send frame data to the writer thread without blocking.
    frame_tx: Option<mpsc::SyncSender<Vec<u8>>>,
    frame_count: u64,
    dropped_frame_count: u64,
    /// SyncClock timestamp (us) of the first frame pushed. Used to offset
    /// synthetic chunk timestamps so they align with the shared clock.
    first_frame_timestamp_us: Option<u64>,
}

impl FfmpegStreamingEncoder {
    pub fn new() -> Self {
        Self {
            child: None,
            chunk_rx: None,
            reader_handle: None,
            writer_handle: None,
            frame_tx: None,
            frame_count: 0,
            dropped_frame_count: 0,
            first_frame_timestamp_us: None,
        }
    }
}

impl StreamingEncoder for FfmpegStreamingEncoder {
    fn start(&mut self, config: StreamingConfig) -> Result<(), StreamingEncoderError> {
        if self.child.is_some() {
            return Err(StreamingEncoderError::AlreadyRunning);
        }

        let ffmpeg_path = super::encoder::find_ffmpeg()
            .map_err(|e| StreamingEncoderError::NotFound(e.to_string()))?;

        info!(
            "Starting streaming encoder: {}x{} @ {}fps",
            config.width, config.height, config.fps
        );

        let codecs = build_codec_list(&ffmpeg_path);
        let mut last_err = None;

        for (codec, codec_args) in &codecs {
            match start_ffmpeg_process(
                &ffmpeg_path,
                &config,
                codec,
                codec_args,
            ) {
                Ok((mut child, chunk_rx, reader_handle)) => {
                    info!("Streaming encoder started with codec: {codec}");

                    // Take stdin for a dedicated writer thread. At 1080p each
                    // frame is ~8MB; blocking write_all on the capture loop
                    // stalls it long enough for ScreenCaptureKit to drop frames.
                    let stdin = child.stdin.take().ok_or_else(|| {
                        StreamingEncoderError::Process("stdin not available".into())
                    })?;

                    const FRAME_CHANNEL_CAPACITY: usize = 8;
                    let (frame_tx, frame_rx) =
                        mpsc::sync_channel::<Vec<u8>>(FRAME_CHANNEL_CAPACITY);

                    let writer_handle = thread::spawn(move || {
                        let mut stdin = stdin;
                        while let Ok(frame_data) = frame_rx.recv() {
                            if let Err(e) = stdin.write_all(&frame_data) {
                                if e.kind() == std::io::ErrorKind::BrokenPipe {
                                    info!("Streaming encoder: stdin pipe closed");
                                } else {
                                    warn!("Streaming encoder: write error: {e}");
                                }
                                break;
                            }
                        }
                    });

                    self.child = Some(child);
                    self.chunk_rx = Some(chunk_rx);
                    self.reader_handle = Some(reader_handle);
                    self.writer_handle = Some(writer_handle);
                    self.frame_tx = Some(frame_tx);
                    self.frame_count = 0;
                    return Ok(());
                }
                Err(e) => {
                    warn!("Streaming encoder failed with {codec}: {e}");
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            StreamingEncoderError::NotFound("no working H.264 encoder found".to_string())
        }))
    }

    fn push_frame(&mut self, frame: &CapturedFrame) -> Result<(), StreamingEncoderError> {
        let tx = self
            .frame_tx
            .as_ref()
            .ok_or(StreamingEncoderError::NotRunning)?;

        // Record SyncClock timestamp of the very first frame
        if self.first_frame_timestamp_us.is_none() {
            self.first_frame_timestamp_us = Some(frame.timestamp_us);
        }

        match tx.try_send(frame.data.clone()) {
            Ok(()) => {
                self.frame_count += 1;
                Ok(())
            }
            Err(mpsc::TrySendError::Full(_)) => {
                self.dropped_frame_count += 1;
                warn!(
                    "Streaming encoder: frame {} dropped (backpressure, total dropped: {})",
                    self.frame_count, self.dropped_frame_count
                );
                self.frame_count += 1;
                Ok(())
            }
            Err(mpsc::TrySendError::Disconnected(_)) => Err(
                StreamingEncoderError::Process("encoder writer thread exited".into()),
            ),
        }
    }

    fn poll_chunk(&mut self) -> Result<Option<EncodedChunk>, StreamingEncoderError> {
        let rx = self.chunk_rx.as_ref().ok_or(StreamingEncoderError::NotRunning)?;
        match rx.try_recv() {
            Ok(chunk) => Ok(Some(chunk)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(StreamingEncoderError::Process("chunk reader disconnected".into()))
            }
        }
    }

    fn stop(&mut self) -> Result<(), StreamingEncoderError> {
        // 1. Drop frame sender → writer thread's recv() returns Err → it exits
        self.frame_tx.take();

        // 2. Join writer thread → its stdin drop signals EOF to FFmpeg
        if let Some(handle) = self.writer_handle.take() {
            let _ = handle.join();
        }

        // 3. Wait for FFmpeg process to finish encoding remaining data
        if let Some(mut child) = self.child.take() {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        debug!("Streaming encoder exited with status: {status}");
                        break;
                    }
                    Ok(None) if std::time::Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Ok(None) => {
                        warn!("Streaming encoder did not exit in time, killing");
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    Err(e) => {
                        warn!("Error waiting for streaming encoder: {e}");
                        break;
                    }
                }
            }
        }

        // 4. Join reader thread
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }

        // Keep chunk_rx alive so callers can drain remaining chunks after stop.
        info!(
            "Streaming encoder stopped (frames sent: {}, frames dropped: {})",
            self.frame_count, self.dropped_frame_count
        );
        self.frame_count = 0;
        self.dropped_frame_count = 0;
        self.first_frame_timestamp_us = None;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.child.is_some()
    }

    fn first_frame_timestamp_us(&self) -> Option<u64> {
        self.first_frame_timestamp_us
    }

    fn dropped_frame_count(&self) -> u64 {
        self.dropped_frame_count
    }
}

/// Build the codec priority list based on what's available.
fn build_codec_list(ffmpeg_path: &str) -> Vec<(String, Vec<String>)> {
    let mut codecs = Vec::new();

    #[cfg(target_os = "macos")]
    {
        if is_codec_available(ffmpeg_path, "h264_videotoolbox") {
            codecs.push((
                "h264_videotoolbox".to_string(),
                vec![
                    "-q:v".to_string(), "65".to_string(),
                    "-allow_sw".to_string(), "1".to_string(),
                    "-realtime".to_string(), "1".to_string(),
                ],
            ));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if is_codec_available(ffmpeg_path, "h264_nvenc") {
            codecs.push((
                "h264_nvenc".to_string(),
                vec![
                    "-preset".to_string(), "p4".to_string(),
                    "-rc".to_string(), "vbr".to_string(),
                    "-cq".to_string(), "23".to_string(),
                ],
            ));
        }
    }

    if is_codec_available(ffmpeg_path, "libx264") {
        codecs.push((
            "libx264".to_string(),
            vec![
                "-preset".to_string(), "ultrafast".to_string(),
                "-crf".to_string(), "23".to_string(),
            ],
        ));
    }

    codecs
}

/// Re-use the codec availability check from encoder module.
fn is_codec_available(ffmpeg_path: &str, codec: &str) -> bool {
    super::encoder::is_codec_available(ffmpeg_path, codec)
}

/// Start an FFmpeg subprocess for streaming fragmented MP4 output.
///
/// Returns the child process, a receiver for encoded chunks, and the
/// reader thread handle.
fn start_ffmpeg_process(
    ffmpeg_path: &str,
    config: &StreamingConfig,
    codec: &str,
    codec_args: &[String],
) -> Result<
    (Child, mpsc::Receiver<EncodedChunk>, thread::JoinHandle<()>),
    StreamingEncoderError,
> {
    let gop_size = (config.fps * GOP_MULTIPLIER).to_string();
    let input_pix_fmt = match config.pixel_format {
        FramePixelFormat::Bgra => "bgra",
        FramePixelFormat::Rgba => "rgba",
    };

    let mut cmd = Command::new(ffmpeg_path);
    cmd.args([
        "-y",
        "-f", "rawvideo",
        "-pix_fmt", input_pix_fmt,
        "-s", &format!("{}x{}", config.width, config.height),
        "-r", &config.fps.to_string(),
        "-i", "pipe:0",
        "-c:v", codec,
    ]);

    for arg in codec_args {
        cmd.arg(arg);
    }

    cmd.args([
        "-pix_fmt", "yuv420p",
        "-g", &gop_size,
        "-movflags", "frag_keyframe+empty_moov+default_base_moof",
        "-f", "mp4",
        "pipe:1",
    ]);

    // Capture stderr for diagnostic logging instead of discarding it
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| StreamingEncoderError::Process(format!("Failed to spawn FFmpeg: {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| StreamingEncoderError::Process("no stdout".to_string()))?;

    // Spawn a thread to drain stderr and log it at debug level
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => debug!("FFmpeg stderr: {}", line),
                    Err(_) => break,
                }
            }
        });
    }

    let (tx, rx) = mpsc::channel();

    // Calculate fragment duration based on GOP size
    let gop_frames = config.fps * GOP_MULTIPLIER;
    let fragment_duration_us = (gop_frames as u64 * 1_000_000) / config.fps as u64;

    // Spawn reader thread that parses MP4 boxes from stdout
    let reader_handle = thread::spawn(move || {
        parse_mp4_boxes(stdout, tx, fragment_duration_us);
    });

    Ok((child, rx, reader_handle))
}

/// Maximum size for a single MP4 box (256 MB) to prevent OOM from corrupted streams.
const MAX_BOX_SIZE: usize = 256 * 1024 * 1024;

/// Parse MP4 boxes from a byte stream and send them as EncodedChunks.
///
/// Accumulates ftyp+moov as a single InitSegment. Each moof+mdat pair
/// becomes a MediaSegment. Handles both regular (4-byte) and extended
/// (8-byte) box sizes.
///
/// `fragment_duration_us` is the expected duration per fragment (typically
/// gop_size / fps * 1_000_000) used for timestamp assignment.
fn parse_mp4_boxes<R: Read>(
    mut reader: R,
    tx: mpsc::Sender<EncodedChunk>,
    fragment_duration_us: u64,
) {
    let mut init_data = Vec::new();
    let mut in_init = true;
    let mut media_buf = Vec::new();
    let mut chunk_timestamp: u64 = 0;
    let timestamp_step: u64 = fragment_duration_us;

    loop {
        // Read box header: 4 bytes size + 4 bytes type
        let mut header = [0u8; 8];
        if reader.read_exact(&mut header).is_err() {
            break;
        }

        let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        let box_type = &header[4..8];
        let box_type_str = String::from_utf8_lossy(box_type).to_string();

        // Handle extended size (size == 1 means 8-byte extended size follows)
        let (total_size, extra_header) = if size == 1 {
            let mut ext = [0u8; 8];
            if reader.read_exact(&mut ext).is_err() {
                break;
            }
            let extended = u64::from_be_bytes(ext);
            (extended, ext.to_vec())
        } else if size == 0 {
            // size == 0 means box extends to EOF — read remaining
            let mut remaining = Vec::new();
            let _ = reader.read_to_end(&mut remaining);
            let _total = 8 + remaining.len() as u64;
            // Assemble full box
            let mut full_box = header.to_vec();
            full_box.extend_from_slice(&remaining);
            // Determine where to put it
            if in_init && (box_type == b"ftyp" || box_type == b"moov") {
                init_data.extend_from_slice(&full_box);
            }
            break;
        } else {
            (size, Vec::new())
        };

        // Read box body — with bounds checks for safety
        let header_overhead = 8 + extra_header.len();
        if (total_size as usize) < header_overhead {
            warn!(
                "Malformed MP4 box '{}': declared size {} < header overhead {}",
                box_type_str, total_size, header_overhead
            );
            break;
        }
        let body_size = total_size as usize - header_overhead;
        if body_size > MAX_BOX_SIZE {
            warn!(
                "MP4 box '{}' body size {} exceeds limit, aborting parse",
                box_type_str, body_size
            );
            break;
        }
        let mut body = vec![0u8; body_size];
        if reader.read_exact(&mut body).is_err() {
            break;
        }

        // Assemble full box bytes
        let mut full_box = header.to_vec();
        full_box.extend_from_slice(&extra_header);
        full_box.extend_from_slice(&body);

        debug!("Parsed MP4 box: {} ({} bytes)", box_type_str, total_size);

        match box_type {
            b"ftyp" | b"moov" => {
                init_data.extend_from_slice(&full_box);
                // After moov, send init segment
                if box_type == b"moov" {
                    in_init = false;
                    let chunk = EncodedChunk {
                        timestamp_us: 0,
                        data: init_data.clone(),
                        chunk_type: ChunkType::InitSegment,
                    };
                    if tx.send(chunk).is_err() {
                        break;
                    }
                }
            }
            b"moof" => {
                // Start of a new media segment
                if !media_buf.is_empty() {
                    // Shouldn't happen (moof without mdat), but flush anyway
                    let chunk = EncodedChunk {
                        timestamp_us: chunk_timestamp,
                        data: std::mem::take(&mut media_buf),
                        chunk_type: ChunkType::MediaSegment,
                    };
                    if tx.send(chunk).is_err() {
                        break;
                    }
                    chunk_timestamp += timestamp_step;
                }
                media_buf.extend_from_slice(&full_box);
            }
            b"mdat" => {
                media_buf.extend_from_slice(&full_box);
                // moof+mdat pair complete — emit as media segment
                let chunk = EncodedChunk {
                    timestamp_us: chunk_timestamp,
                    data: std::mem::take(&mut media_buf),
                    chunk_type: ChunkType::MediaSegment,
                };
                if tx.send(chunk).is_err() {
                    break;
                }
                chunk_timestamp += timestamp_step;
            }
            _ => {
                // Unknown box in init phase goes to init_data
                if in_init {
                    init_data.extend_from_slice(&full_box);
                } else {
                    // Non-standard box after init — append to current media buffer
                    media_buf.extend_from_slice(&full_box);
                }
            }
        }
    }

    // Flush any remaining media data
    if !media_buf.is_empty() {
        let chunk = EncodedChunk {
            timestamp_us: chunk_timestamp,
            data: media_buf,
            chunk_type: ChunkType::MediaSegment,
        };
        let _ = tx.send(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::encoded_ring_buffer::ChunkType;

    // T7: FfmpegStreamingEncoder start/stop (skip if no FFmpeg)
    #[test]
    fn streaming_encoder_start_stop() {
        if super::super::encoder::find_ffmpeg().is_err() {
            eprintln!("Skipping test: FFmpeg not found");
            return;
        }

        let mut encoder = FfmpegStreamingEncoder::new();
        let config = StreamingConfig {
            width: 64,
            height: 64,
            fps: 30,
            pixel_format: FramePixelFormat::Rgba,
        };

        encoder.start(config).unwrap();
        assert!(encoder.is_running());

        encoder.stop().unwrap();
        assert!(!encoder.is_running());
    }

    // T8: push frames then poll yields chunks (skip if no FFmpeg)
    #[test]
    fn push_frames_poll_yields_chunks() {
        if super::super::encoder::find_ffmpeg().is_err() {
            eprintln!("Skipping test: FFmpeg not found");
            return;
        }

        let mut encoder = FfmpegStreamingEncoder::new();
        let config = StreamingConfig {
            width: 64,
            height: 64,
            fps: 30,
            pixel_format: FramePixelFormat::Rgba,
        };
        encoder.start(config).unwrap();

        // Push enough frames and collect chunks inline
        let mut chunks = Vec::new();
        for i in 0..90 {
            let frame = CapturedFrame {
                timestamp_us: i * 33_333,
                width: 64,
                height: 64,
                data: vec![255, 0, 0, 255].repeat(64 * 64),
                pixel_format: FramePixelFormat::Rgba,
            };
            if encoder.push_frame(&frame).is_err() {
                break;
            }
            while let Ok(Some(chunk)) = encoder.poll_chunk() {
                chunks.push(chunk);
            }
        }

        // Stop flushes remaining data; drain remaining chunks after stop
        encoder.stop().unwrap();
        while let Ok(Some(chunk)) = encoder.poll_chunk() {
            chunks.push(chunk);
        }

        assert!(!chunks.is_empty(), "should have received at least one chunk");
        // Verify we got an init segment
        assert!(
            chunks.iter().any(|c| c.chunk_type == ChunkType::InitSegment),
            "should have received an init segment"
        );
    }

    // T9: init segment starts with ftyp box
    #[test]
    fn init_segment_starts_with_ftyp() {
        if super::super::encoder::find_ffmpeg().is_err() {
            eprintln!("Skipping test: FFmpeg not found");
            return;
        }

        let mut encoder = FfmpegStreamingEncoder::new();
        let config = StreamingConfig {
            width: 64,
            height: 64,
            fps: 30,
            pixel_format: FramePixelFormat::Rgba,
        };
        encoder.start(config).unwrap();

        // Push frames until we get an init segment
        let mut got_init = false;
        for i in 0..120 {
            let frame = CapturedFrame {
                timestamp_us: i * 33_333,
                width: 64,
                height: 64,
                data: vec![255, 0, 0, 255].repeat(64 * 64),
                pixel_format: FramePixelFormat::Rgba,
            };
            if encoder.push_frame(&frame).is_err() {
                break;
            }

            // Poll for chunks
            while let Ok(Some(chunk)) = encoder.poll_chunk() {
                if chunk.chunk_type == ChunkType::InitSegment {
                    // Verify ftyp box at start
                    assert!(
                        chunk.data.len() >= 8,
                        "init segment too small: {} bytes",
                        chunk.data.len()
                    );
                    assert_eq!(
                        &chunk.data[4..8],
                        b"ftyp",
                        "init segment should start with ftyp box"
                    );
                    got_init = true;
                }
            }

            if got_init {
                break;
            }
        }

        // Stop flushes remaining frames; drain chunks that arrived after
        // the push loop (writer thread adds latency).
        encoder.stop().unwrap();
        while let Ok(Some(chunk)) = encoder.poll_chunk() {
            if chunk.chunk_type == ChunkType::InitSegment {
                assert!(
                    chunk.data.len() >= 8,
                    "init segment too small: {} bytes",
                    chunk.data.len()
                );
                assert_eq!(
                    &chunk.data[4..8],
                    b"ftyp",
                    "init segment should start with ftyp box"
                );
                got_init = true;
            }
        }
        assert!(got_init, "should have received an init segment");
    }

    // T10: returns NotFound error when FFmpeg absent (bad path)
    #[test]
    fn returns_not_found_with_bad_ffmpeg_path() {
        // Set env var to a nonexistent path
        let original = std::env::var("GAMECLIP_FFMPEG_PATH").ok();
        std::env::set_var("GAMECLIP_FFMPEG_PATH", "/nonexistent/ffmpeg");

        let mut encoder = FfmpegStreamingEncoder::new();
        let config = StreamingConfig {
            width: 64,
            height: 64,
            fps: 30,
            pixel_format: FramePixelFormat::Rgba,
        };
        let result = encoder.start(config);

        // Restore env
        match original {
            Some(val) => std::env::set_var("GAMECLIP_FFMPEG_PATH", val),
            None => std::env::remove_var("GAMECLIP_FFMPEG_PATH"),
        }

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, StreamingEncoderError::NotFound(_)),
            "expected NotFound, got: {err}"
        );
    }

    // Test: parse_mp4_boxes with synthetic data
    #[test]
    fn parse_mp4_boxes_synthetic() {
        // Build synthetic ftyp + moov + moof + mdat
        let mut stream = Vec::new();

        // ftyp box (size=12, type=ftyp, body=4 bytes)
        stream.extend_from_slice(&[0, 0, 0, 12]); // size
        stream.extend_from_slice(b"ftyp");          // type
        stream.extend_from_slice(&[0, 0, 0, 0]);   // body

        // moov box (size=12)
        stream.extend_from_slice(&[0, 0, 0, 12]);
        stream.extend_from_slice(b"moov");
        stream.extend_from_slice(&[0, 0, 0, 0]);

        // moof box (size=12)
        stream.extend_from_slice(&[0, 0, 0, 12]);
        stream.extend_from_slice(b"moof");
        stream.extend_from_slice(&[0, 0, 0, 0]);

        // mdat box (size=16)
        stream.extend_from_slice(&[0, 0, 0, 16]);
        stream.extend_from_slice(b"mdat");
        stream.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);

        let cursor = std::io::Cursor::new(stream);
        let (tx, rx) = mpsc::channel();
        parse_mp4_boxes(cursor, tx, 2_000_000); // 2s fragment duration for test

        let chunks: Vec<EncodedChunk> = rx.try_iter().collect();
        assert_eq!(chunks.len(), 2, "expected init + 1 media segment");
        assert_eq!(chunks[0].chunk_type, ChunkType::InitSegment);
        assert_eq!(chunks[1].chunk_type, ChunkType::MediaSegment);

        // Init should contain ftyp + moov
        let init = &chunks[0].data;
        assert!(init.len() >= 8);
        assert_eq!(&init[4..8], b"ftyp");
    }
}
