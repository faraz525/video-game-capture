# GameClip Architecture Codemap

> Freshness: 2026-02-26 | 183 Rust tests, 3 frontend tests | Branch: feat/production-readiness

## System Overview

Tauri v2 monorepo: Rust backend captures screen/input/audio, React+TypeScript frontend displays clips. macOS dev, Windows production target.

```
gameclip/
  apps/desktop/
    src-tauri/src/          # Rust backend (37 .rs files, 9 modules)
    src/                    # React frontend (15 files)
  Cargo.toml               # Workspace root
  package.json              # pnpm workspace root
```

## Module Dependency Tiers

```
Tier 0 (leaf — no crate deps):
  clip/metadata, sync/clock, sync/ring_buffer, input/mod, audio/mod,
  capture/mod, annotation/types, upload/progress, game/detector

Tier 1 (depends on Tier 0):
  capture/*, input/*, audio/*, sync/encoded_ring_buffer, upload/error, upload/privacy

Tier 2 (depends on Tier 0-1):
  clip/encoder, clip/streaming, annotation/frame_actions, annotation/quality

Tier 3 (depends on Tier 0-2):
  clip/format, clip/saver, annotation/export, annotation/mod

Tier 4 (orchestrators):
  engine, commands, upload/hf_client

Tier 5 (root):
  lib.rs
```

## Data Flow

```
Capture thread (engine::start_capture)
  ScreenCapture::poll_frame() -> CapturedFrame
    -> ClipSaver::push_frame()                    [Arc<Mutex<ClipSaver>>]
    -> FfmpegStreamingEncoder::push_frame()       [dedicated writer thread]
    -> ClipSaver::push_encoded_chunk()            [Arc<Mutex<ClipSaver>>]
  InputRecorder::poll_events() -> Vec<InputEvent>
    -> ClipSaver::push_input()                    [Arc<Mutex<ClipSaver>>]
  AudioCapture::poll_buffer() -> AudioBuffer
    -> ClipSaver::push_audio()                    [Arc<Mutex<ClipSaver>>]

Hotkey (Ctrl+Shift+R) / save_clip command
  engine::save_clip(Arc<Mutex<ClipSaver>>)
    -> ClipSaver::save_clip(dir)
      -> EncodedRingBuffer::drain_as_fmp4() OR encode_frames_to_mp4()
      -> annotation::annotate_from_events() -> ClipAnnotations
      -> write_clip(path, ClipPackageData)
    -> emits "clip-saved" event to webview

Frontend IPC
  list_clips       -> read_clip_metadata() per file   [fast zip read]
  get_clip_thumbnail -> read_clip_thumbnail()          [fast zip read]
  extract_clip_video -> read_clip() -> temp MP4        [re-encode if raw]
  upload_clips     -> upload::hf_client::upload_clips() -> "upload-progress" events
```

## Shared State Across Thread Boundaries

| Value | Type | Owner -> Consumer |
|---|---|---|
| ClipSaver | `Arc<Mutex<ClipSaver>>` | capture thread push -> save_clip drain |
| running | `Arc<AtomicBool>` | lib.rs -> engine capture loop |
| upload_cancel | `Mutex<Arc<AtomicBool>>` | cancel_upload cmd -> upload_clips loop |
| AppSettings | `Mutex<AppSettings>` | update_settings -> get_settings / engine |
| SyncClock epoch | `Arc<Instant>` | shared across capture/input/audio |
| Encoder stdin | `mpsc::SyncSender<Vec<u8>>` | push_frame -> writer thread |

## Trait Hierarchy

```
ScreenCapture -> MockCapture | MacOSCapture | WindowsCapture
InputRecorder -> MockInputRecorder | MacOSInputRecorder | WindowsInputRecorder
AudioCapture  -> MockAudioCapture | WindowsAudioCapture
StreamingEncoder -> FfmpegStreamingEncoder
Timestamped   -> CapturedFrame | InputEvent | AudioBuffer | EncodedChunk
```

Platform selection: `#[cfg(target_os)]` gated factory functions in `engine.rs`.

## Tauri Integration

**Managed State:** `EngineState` (saver, running, settings, upload_cancel)

**Plugins:** `tauri_plugin_opener`, `tauri_plugin_shell`, `tauri_plugin_global_shortcut`

**Events:**
- `"clip-saved"` (backend -> frontend): file path string
- `"upload-progress"` (backend -> frontend): `UploadProgress` struct

**15 IPC Commands:** list_clips, get_clip_metadata, delete_clip, save_clip, get_settings, update_settings, extract_clip_video, get_clip_thumbnail, get_clip_input_events, annotate_clip, get_frame_actions, get_quality_score, export_clips, upload_clips, cancel_upload

## Key Design Decisions

1. **Streaming encoding** — Frames encoded in capture thread via FFmpeg subprocess, stored as fMP4 chunks in `EncodedRingBuffer`. Save is drain+concatenate, not re-encode.
2. **Platform abstraction** — Trait per subsystem, cfg-gated impls. Mock impls for dev/test.
3. **FIFO drain loop** — Capture polls all available frames per iteration with burst cap (fps/5).
4. **Non-blocking encoder writes** — Dedicated writer thread prevents 8MB frame writes from blocking capture.
5. **Fast zip reads** — `read_clip_metadata()` and `read_clip_thumbnail()` extract single entries without decompressing entire archive.
6. **Immutable patterns** — `scrub_metadata()` returns new ClipMetadata, never mutates. Settings page uses draft-state pattern.
