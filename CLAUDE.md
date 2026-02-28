# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Development Commands

| Task | Command |
|------|---------|
| Dev (Tauri + Vite) | `pnpm tauri dev` |
| Dev (frontend only) | `pnpm dev` |
| Build frontend | `pnpm build` |
| Run all Rust tests | `cargo test` |
| Run single test | `cargo test <test_name>` |
| Run module tests | `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml <module_path>` |
| Run frontend tests | `cd apps/desktop && pnpm exec vitest run` |
| TypeScript check | `cd apps/desktop && npx tsc --noEmit` |
| Rust lint | `cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml` |
| Download FFmpeg sidecar | `./scripts/download-ffmpeg.sh` |
| Cross-compile Windows | `cargo xwin build --target x86_64-pc-windows-msvc --release` |

Vite dev server runs on port 1420 (hardcoded in `vite.config.ts`).

## Architecture

Tauri v2 monorepo: Rust backend captures screen/input/audio, React+TypeScript frontend displays clips. Developed on Mac with platform-specific implementations; real production capture targets Windows.

### Rust Backend (`apps/desktop/src-tauri/src/`)

**Platform abstraction pattern** — Each capture subsystem has a trait + platform-specific implementations:
- `capture/mod.rs` defines `ScreenCapture` trait → `capture/mock.rs` (color-cycling RGBA) / `capture/macos.rs` (ScreenCaptureKit) / `capture/windows.rs` (DXGI Desktop Duplication)
- `input/mod.rs` defines `InputRecorder` trait → `input/mock.rs` (WASD/mouse events) / `input/macos.rs` (CGEventTap) / `input/windows.rs` (Raw Input API with RIDEV_INPUTSINK)
- `audio/mod.rs` defines `AudioCapture` trait → `audio/mock.rs` (440Hz sine wave) / `audio/windows.rs` (WASAPI loopback)
- Platform impls are `#[cfg(target_os)]` gated; `engine.rs` uses cfg-gated factory functions to select the right implementation at compile time

**macOS requirements:** The `input/macos.rs` CGEventTap requires Accessibility permission (System Settings > Privacy & Security > Accessibility). The `capture/macos.rs` ScreenCaptureKit requires Screen Recording permission.

**Data flow:** Capture sources → `Arc<Mutex<ClipSaver>>` (capture thread pushes directly) → `RingBuffer<T: Timestamped>` (last N seconds) → `save_clip()` drains buffers → `write_clip()` produces `.gameclip` zip archive.

**Key modules:**
- `engine.rs` — `EngineState` (managed Tauri state): owns `Arc<Mutex<ClipSaver>>`, `AppSettings`, `upload_cancel: Arc<AtomicBool>`, spawns capture thread. Uses cfg-gated factory functions to select platform implementations. Integrates game detection via `detect_current_game()`. Capture loop uses FIFO drain (all available frames per iteration), two-phase frame pacing with `max_burst` cap, and a dedicated writer thread for FFmpeg stdin to avoid blocking.
- `clip/encoder.rs` — `FfmpegEncoder` subprocess: pipes raw BGRA frames via stdin to FFmpeg. Platform-aware codec priority: macOS uses `h264_videotoolbox` → `libx264`, Windows uses `h264_nvenc` → `libx264`. `reencode_raw_to_mp4()` re-encodes raw RGBA clips on playback. FFmpeg path resolution: (1) `GAMECLIP_FFMPEG_PATH` env, (2) bundled sidecar, (3) exe-adjacent, (4) well-known paths, (5) system PATH.
- `clip/streaming.rs` — `StreamingEncoder` trait + `FfmpegStreamingEncoder`: pipes frames to FFmpeg during capture for real-time encoding. Produces fragmented MP4 chunks stored in `EncodedRingBuffer`. Falls back to raw RGBA when FFmpeg unavailable. `GOP_MULTIPLIER = 2` (keyframe every 2 seconds).
- `game/detector.rs` — Game detection: foreground window check (Windows-only) + process scan (cross-platform via `sysinfo`). Matches against ~30 known game process names. Also provides `game_to_genre()` mapping used by quality scoring.
- `commands.rs` — Tauri IPC commands: `list_clips`, `get_clip_metadata`, `delete_clip`, `save_clip`, `get_settings`, `update_settings`, `extract_clip_video`, `get_clip_thumbnail`, `get_clip_input_events`, `annotate_clip`, `get_frame_actions`, `get_quality_score`, `export_clips`, `upload_clips`, `cancel_upload`
- `lib.rs` — App entry: registers state (`EngineState`), plugins (`opener`, `shell`, `global-shortcut`), system tray ("Settings"/"Quit"), global shortcut (`Ctrl+Shift+R`), IPC handlers (15 commands). Logging via `simplelog` to `~/GameClip/gameclip.log` + terminal. On hotkey: saves clip and emits `"clip-saved"` event to webview.
- `sync/ring_buffer.rs` — Generic `RingBuffer<T>` backed by `VecDeque`, evicts by `max_duration_us`
- `sync/encoded_ring_buffer.rs` — `EncodedRingBuffer` for storing encoded video chunks from streaming encoder. Time-based eviction, `drain_as_fmp4()` concatenation, first-frame thumbnail cache.
- `sync/clock.rs` — `SyncClock` wraps `Arc<Instant>`, derives `Clone` so all clones share the same epoch for cross-stream timestamp synchronization
- `clip/format.rs` — `.gameclip` read/write. Zip containing `metadata.json`, `input.jsonl`, `video.bin`, optional `audio.bin`, `thumbnail.jpg`, `frame_actions.jsonl`, `quality.json`
- `clip/saver.rs` — `ClipSaver` holds three ring buffers, assembles clip data, generates metadata
- `clip/metadata.rs` — `ClipMetadata` serde struct with `CaptureDevices`, includes `annotation_layers` and `video_start_timestamp_us`

**Annotation pipeline (`annotation/`)** — Transforms raw `.gameclip` data into ML-ready annotations for world model training:
- `annotation/types.rs` — `FrameAction` (per-frame input state snapshot), `QualityScore` (genre-weighted clip scoring), `ClipAnnotations`, `AnnotationManifest`
- `annotation/frame_actions.rs` — State machine that converts event-stream into per-frame action vectors (keys_held, mouse position/delta, scroll delta). Output format matches what world model papers (GF-Minecraft, DIAMOND, GameNGen) require.
- `annotation/quality.rs` — Genre-aware quality scoring with dimension scores (action_density, input_continuity, input_diversity, mouse_control, action_complexity, highlight_density). `GenreWeights` applies different weights per game genre (FPS, racing, MOBA, etc.).
- `annotation/export.rs` — Export to `JsonSidecar` (MP4 + JSON per clip) or `HuggingfaceDataset` (Datasets-compatible directory structure for PyTorch DataLoader).
- `annotation/mod.rs` — Orchestrates pipeline: `annotate_clip()` (from file) and `annotate_from_events()` (from memory, used during clip save).

**Upload pipeline (`upload/`)** — One-click upload of annotated clips to HuggingFace dataset repos:
- `upload/hf_client.rs` — `HfClient` REST client using reqwest. `ensure_repo_exists()`, `upload_clip()` (NDJSON commit), `prepare_clip()` (read/scrub/quality check), `upload_clips()` (orchestrates multi-clip upload with cancellation).
- `upload/privacy.rs` — `scrub_metadata()` returns new ClipMetadata with PII redacted (path separators stripped). Never mutates input.
- `upload/progress.rs` — `UploadProgress` and `UploadStage` types emitted via Tauri events.
- `upload/error.rs` — `HfError` enum covering HTTP, auth, quality gate, consent, and I/O errors.

### Frontend (`apps/desktop/src/`)

Simple page router in `App.tsx` using `useState<"library" | "player" | "settings">`.

**Hooks (Tauri IPC bridge):**
- `useClips` — calls `invoke()` for clip CRUD, listens for `"clip-saved"` events from Rust
- `useClipData` — loads clip video (via `extract_clip_video`) and input events (via `get_clip_input_events`), normalizes timestamps against `video_start_timestamp_us`
- `useThumbnail` — loads base64 thumbnail via `get_clip_thumbnail`
- `useSettings` — calls `invoke()` for settings get/update (includes `HuggingFaceConfig`)
- `useUpload` — listens to `upload-progress` Tauri events, exposes `uploadClips()`, `cancelUpload()`, progress state

**Pages:** `ClipLibrary` (grid of ClipCards), `ClipPlayer` (rAF-based playback with InputOverlay), `Settings` (form with draft state pattern — capture settings, storage, and HuggingFace upload config with consent/quality gate/private repo toggles).

**`InputOverlay`** — Renders active keys, mouse cursor, and click ripples synced to playback time using 500ms visibility windows.

### `.gameclip` Format

Zip archive containing:
- `metadata.json` — clip id, game, resolution, fps, duration, device flags, timestamps, annotation_layers, format_version (v2), checksums (SHA-256 per entry)
- `input.jsonl` — one `InputEvent` per line (tagged union: Key, MouseButton, MouseMove, MouseScroll)
- `video.bin` — raw RGBA (mock/fallback) or H.264 MP4 (encoded)
- `audio.bin` — raw PCM f32 LE (optional)
- `thumbnail.jpg` — first frame data (optional)
- `frame_actions.jsonl` — per-frame action snapshots (optional, from annotation pipeline)
- `quality.json` — clip quality score (optional, from annotation pipeline)

## Known Limitations

- Windows capture implementations need testing on actual Windows hardware.
- FFmpeg sidecar binary must be downloaded via `./scripts/download-ffmpeg.sh` before `pnpm tauri dev` (or symlinked locally: `ln -sf $(which ffmpeg) apps/desktop/src-tauri/binaries/ffmpeg-$(rustc -vV | grep host | cut -d' ' -f2)`).

## Testing

183 Rust unit tests, all co-located with source (`#[cfg(test)] mod tests`). 3 frontend tests (Vitest + @testing-library/react) in `src/hooks/useUpload.test.ts`.

Rust tests use `tempfile::TempDir` for filesystem operations and `std::thread::sleep` for timing. Pattern: test state machine transitions (start/stop), error conditions (double-start, poll-without-start), data integrity, serialization round-trips, annotation pipeline correctness, upload privacy scrubbing, and HTTP mocking via `mockito`. Frontend tests mock `@tauri-apps/api/core` and `@tauri-apps/api/event` (configured in `src/test-setup.ts`).

## Implementation Status

Steps 1–10 complete: scaffolding, platform abstraction, ring buffer + clip save, clip library UI, data annotation pipeline, Windows capture engine (untested on hardware), streaming encoding, FFmpeg sidecar bundling, format versioning with SHA-256 checksums, and HuggingFace upload pipeline. Recent work on `feat/production-readiness` branch added 1080p capture, clock-based frame pacing, FIFO drain loop, non-blocking encoder writes, and fast clip library loading with targeted zip reads and thumbnail caching. See `PLAN.md` for full roadmap.
