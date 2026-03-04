# GameClip Backend Codemap

> Freshness: 2026-02-26 | 183 Rust tests | Path: apps/desktop/src-tauri/src/

## Module Map

```
src/
  lib.rs              — App entry, plugins, tray, hotkey, IPC registration
  engine.rs           — EngineState, AppSettings, capture loop, save_clip
  commands.rs         — 15 Tauri IPC commands, ClipSummary, ClipInputData
  main.rs             — binary entrypoint
  capture/
    mod.rs            — ScreenCapture trait, CapturedFrame, FramePixelFormat
    mock.rs           — Color-cycling RGBA frames
    macos.rs          — ScreenCaptureKit (BGRA output)
    windows.rs        — DXGI Desktop Duplication (BGRA output)
  input/
    mod.rs            — InputRecorder trait, InputEvent (tagged union)
    mock.rs           — Synthetic WASD/mouse events
    macos.rs          — CGEventTap (requires Accessibility permission)
    windows.rs        — Raw Input API with RIDEV_INPUTSINK
  audio/
    mod.rs            — AudioCapture trait, AudioBuffer
    mock.rs           — 440Hz sine wave
    windows.rs        — WASAPI loopback
  sync/
    clock.rs          — SyncClock (Arc<Instant> shared epoch), TimestampUs
    ring_buffer.rs    — Generic RingBuffer<T: Timestamped>, VecDeque-backed
    encoded_ring_buffer.rs — EncodedRingBuffer for fMP4 chunks, thumbnail cache
  clip/
    encoder.rs        — FfmpegEncoder (batch), find_ffmpeg(), codec detection
    streaming.rs      — StreamingEncoder trait, FfmpegStreamingEncoder (real-time)
    format.rs         — .gameclip zip read/write, checksums, migration v1->v2
    metadata.rs       — ClipMetadata, CaptureDevices (serde structs)
    saver.rs          — ClipSaver (ring buffer owner, save orchestrator)
  annotation/
    mod.rs            — annotate_clip(), annotate_from_events() orchestration
    types.rs          — FrameAction, QualityScore, DimensionScores, ClipAnnotations
    frame_actions.rs  — Event-stream -> per-frame action vector state machine
    quality.rs        — Genre-aware scoring (6 dimensions + genre weights)
    export.rs         — JsonSidecar + HuggingfaceDataset export formats
  game/
    mod.rs            — re-exports detector
    detector.rs       — detect_current_game(), game_to_genre(), KNOWN_GAMES (~30)
  upload/
    mod.rs            — re-exports error, hf_client, privacy, progress
    hf_client.rs      — HfClient REST client, HuggingFaceConfig, upload_clips()
    privacy.rs        — scrub_metadata() (PII redaction, returns new struct)
    error.rs          — HfError enum (thiserror, serializes as string)
    progress.rs       — UploadProgress, UploadStage (Tauri event payload)
```

## Per-Module Public API

### engine.rs
- `EngineState` — Tauri managed state: saver, running, settings, upload_cancel
- `AppSettings` — Runtime config: fps, resolution, buffer_duration, save_dir, hotkey, huggingface
- `create_engine_state() -> EngineState`
- `start_capture(state, app)` — Spawns capture thread with FIFO drain loop
- `save_clip(saver) -> Result<PathBuf>` — Drains buffers, encodes, writes .gameclip

### commands.rs
- `ClipSummary` — Frontend-facing flat struct (no video bytes)
- `ClipInputData` — events + video_start_timestamp_us for playback sync
- 15 commands: 4 async (save_clip, extract_clip_video, export_clips, upload_clips), 11 sync

### clip/encoder.rs
- `FfmpegEncoder::start(path, w, h, fps, fmt)` — Codec cascade: VideoToolbox/NVENC -> libx264
- `encode_frames_to_mp4(frames, fps) -> Vec<u8>` — Batch encode
- `reencode_raw_to_mp4(data, w, h, fps) -> Vec<u8>` — Legacy raw RGBA re-encode
- `find_ffmpeg() -> Option<PathBuf>` — env -> sidecar -> exe-adjacent -> well-known -> PATH
- `set_sidecar_path(path)` — OnceLock for Tauri sidecar resolution
- `is_codec_available(path, codec) -> bool`

### clip/streaming.rs
- `StreamingEncoder` trait: start, push_frame, poll_chunk, stop, is_running
- `FfmpegStreamingEncoder` — mpsc channels, dedicated writer/reader threads
- `StreamingConfig` — fps, width, height, pixel_format
- `GOP_MULTIPLIER = 2` (keyframe every 2s)

### clip/format.rs
- `write_clip(path, data)` — Zip with SHA-256 checksums (v2)
- `read_clip(path) -> ClipPackageContents` — Full deserialize
- `read_clip_metadata(path)` — Fast: metadata.json only
- `read_clip_thumbnail(path)` — Fast: thumbnail.jpg only
- `migrate_v1_to_v2(path)` — Adds checksums in-place

### clip/saver.rs
- `ClipSaver` — Owns raw ring buffers + optional EncodedRingBuffer
- `push_frame/push_input/push_audio/push_encoded_chunk` — Capture thread entry points
- `save_clip(save_dir) -> Result<PathBuf>` — Drain + annotate + write
- `enable_encoded_buffer(buf)` — Attach streaming encoder output
- `cache_first_raw_frame(frame)` — Thumbnail source

### annotation pipeline
- `index_frame_actions(events, fps, duration, video_start_us) -> Vec<FrameAction>`
- `score_clip_quality(events, fps, duration, game) -> QualityScore`
- `export_clip_json_sidecar(clip_path, out_dir)` — MP4 + JSON per clip
- `export_dataset_huggingface(paths, out_dir)` — HF Datasets-compatible layout

### upload pipeline
- `HfClient::ensure_repo_exists()` / `upload_clip()` / `upload_clips()`
- `prepare_clip(path) -> (contents, quality, name)` — Read + score + scrub
- `scrub_metadata(meta, opts) -> ClipMetadata` — PII-free copy

## External Crate Usage

| Domain | Crates |
|---|---|
| App framework | tauri 2, tauri-plugin-{opener,shell,global-shortcut} |
| Serialization | serde 1, serde_json 1, chrono 0.4 |
| IDs | uuid 1 (v4) |
| Errors | thiserror 2 |
| Archive | zip 2 |
| Image | image 0.25 |
| Integrity | sha2 0.10, hex 0.4 |
| HTTP | reqwest 0.12 (blocking, rustls-tls) |
| Encoding | base64 0.22 |
| Process scan | sysinfo 0.38 |
| Logging | log 0.4, simplelog 0.12 |
| macOS | screencapturekit 1.1.0, core-graphics 0.24, core-foundation 0.10 |
| Windows | win_desktop_duplication 0.10, wasapi 0.22, windows 0.59 |
| Test | tempfile 3, mockito 1 |

## Test Distribution

| Module | Tests |
|---|---|
| annotation | 50 |
| clip (encoder, format, metadata, saver, streaming) | 42 |
| sync (clock, encoded_ring_buffer, ring_buffer) | 26 |
| input (mock, macos) | 19 |
| upload (hf_client, privacy, progress) | 17 |
| game (detector) | 13 |
| capture (mock) | 8 |
| audio (mock) | 8 |
| **Total** | **183** |
