# GameClip — Implementation Plan

## Vision

GameClip is an open-source, lightweight desktop capture tool that records gameplay clips with synchronized input data. It is the distribution vehicle for the world's first open dataset of frame-action-annotated gameplay — training data that world model researchers (GameNGen, DIAMOND, Genie, NitroGen) desperately need.

**Strategy:** Open source tool for adoption → community-contributed data → default dataset for world model training → enterprise/API monetization.

**Key insight:** No existing tool bridges "game clip capture for gamers" and "training data pipeline for AI researchers." Medal.tv ($333M valuation) captures video for sharing but has no input annotations. OpenAI paid $80/hr for labeled Minecraft gameplay. GameClip produces that data format for free, at scale.

---

## Architecture

Tauri v2 monorepo: Rust backend captures screen/input/audio, React+TypeScript frontend displays clips. Developed on Mac with platform-specific implementations; real production capture targets Windows.

```
gameclip/
├── apps/
│   └── desktop/
│       ├── src-tauri/src/
│       │   ├── capture/      # Platform-abstracted screen capture (trait + mock/macos/windows)
│       │   ├── input/        # Input recording (trait + mock/macos/windows)
│       │   ├── audio/        # Audio capture (trait + mock/windows)
│       │   ├── sync/         # SyncClock + RingBuffer<T>
│       │   ├── clip/         # .gameclip format, saver, encoder
│       │   ├── game/         # Game detection (process scan + genre mapping)
│       │   ├── annotation/   # ML annotation pipeline (frame_actions, quality, export)
│       │   ├── engine.rs     # EngineState, capture loop, save_clip
│       │   ├── commands.rs   # 13 Tauri IPC commands
│       │   └── lib.rs        # App entry, tray, hotkey, IPC registration
│       └── src/              # React + TypeScript frontend
│           ├── pages/        # ClipLibrary, ClipPlayer, Settings
│           ├── components/   # ClipCard, InputOverlay
│           └── hooks/        # useClips, useClipData, useThumbnail, useSettings
├── Cargo.toml                # Workspace root
└── package.json              # pnpm workspace root
```

---

## Completed Steps

### Step 1: Project Scaffolding ✅
Tauri app boots, system tray, global hotkey (Ctrl+Shift+R).

### Step 2: Platform Abstraction Layer ✅
Traits for ScreenCapture, InputRecorder, AudioCapture, SyncClock. Mock + macOS + Windows implementations. 28 tests.

### Step 3: Ring Buffer + Clip Save ✅
Time-bounded ring buffer, .gameclip zip format, hotkey-triggered save, FFmpeg encoding with fallback. 23 tests.

### Step 4: Clip Library UI ✅
Clip grid, player with input overlay, settings page, Tauri IPC bridge. TypeScript clean.

### Step 5: Data Annotation Pipeline ✅
Frame-action state machine, genre-aware quality scoring, JSON sidecar + HuggingFace export. 51 tests.

### Step 6: Windows Capture Engine (partial)
DXGI, Raw Input, WASAPI code written but untested on real Windows hardware. macOS implementations (ScreenCaptureKit, CGEventTap) working.

**Current: 128 Rust tests, 0 frontend tests.**

---

## Next Steps — Production Readiness

### Step 7: In-Capture Encoding (Memory Fix) 🔴 CRITICAL

**Problem:** The ring buffer stores raw RGBA frames. At 1080p60 for 30s, that's ~14GB of RAM. Even at 640x360, it's ~830MB. This makes the tool unusable on most gaming PCs.

**Solution:** Encode frames in the capture thread using a streaming encoder. Store compressed H.264 chunks in the ring buffer instead of raw RGBA. Target: <200MB memory at 1080p60/30s.

**Design:**
- New `EncodedChunk` struct: timestamp range, H.264 data, keyframe flag
- `StreamingEncoder` trait: feed raw frames → get encoded chunks
- `FfmpegStreamingEncoder`: long-running FFmpeg subprocess, segment output
- `RingBuffer<EncodedChunk>` replaces `RingBuffer<CapturedFrame>` in ClipSaver
- On save: concatenate chunks, write MP4 container, no re-encoding needed
- Thumbnail extraction from first keyframe
- Fallback: if encoder unavailable, keep raw RGBA at lower resolution

**Key constraint:** The encoder must run continuously (not just at save time), accepting frames from the capture thread and emitting chunks. Save becomes a simple "drain and concatenate" operation.

**Files to modify:**
- `clip/encoder.rs` — add `StreamingEncoder` trait + `FfmpegStreamingEncoder`
- `clip/saver.rs` — new `EncodedChunk` type, swap `RingBuffer<CapturedFrame>` for `RingBuffer<EncodedChunk>`
- `engine.rs` — create encoder at capture start, pipe frames through encoder before ring buffer
- `clip/format.rs` — handle pre-encoded video data in write_clip
- `commands.rs` — update extract_clip_video for pre-encoded clips

**Checkpoint 7:**
- [ ] `StreamingEncoder` trait defined with comprehensive tests
- [ ] `FfmpegStreamingEncoder` passes frames and produces valid H.264 chunks
- [ ] `RingBuffer<EncodedChunk>` correctly evicts old chunks by timestamp
- [ ] Memory usage at 1080p60/30s stays under 200MB (measured with test)
- [ ] Save operation concatenates chunks into valid MP4
- [ ] Thumbnail still generated from first frame (decoded from keyframe or cached)
- [ ] Fallback to raw RGBA works when FFmpeg unavailable
- [ ] All existing tests still pass
- [ ] `cargo test` passes with new encoding pipeline

---

### Step 8: Bundle FFmpeg as Tauri Sidecar 🔴 CRITICAL

**Problem:** FFmpeg must be in the user's system PATH. Most gamers don't have FFmpeg installed. This kills adoption.

**Solution:** Bundle FFmpeg as a Tauri sidecar binary. Tauri v2 has first-class sidecar support — binaries in `src-tauri/binaries/` are bundled with the app.

**Design:**
- Download platform-specific FFmpeg static builds (GPL or LGPL depending on license choice)
- Place in `src-tauri/binaries/ffmpeg-{target_triple}` (Tauri naming convention)
- Update `find_ffmpeg()` in `encoder.rs` to check sidecar path first (already partially implemented)
- Update `tauri.conf.json` to declare the sidecar in `bundle.externalBin`
- Add a build script or Makefile target to download FFmpeg binaries for each platform
- Document LGPL compliance if using LGPL build

**Files to modify:**
- `src-tauri/tauri.conf.json` — add `bundle.externalBin` config
- `clip/encoder.rs` — update `find_ffmpeg()` to use Tauri sidecar API
- `scripts/download-ffmpeg.sh` — new script to fetch platform binaries
- `Cargo.toml` — if needed for build script

**Checkpoint 8:**
- [ ] FFmpeg sidecar binary bundled in dev build
- [ ] `find_ffmpeg()` resolves sidecar path first
- [ ] Encoding works with bundled FFmpeg (no system PATH needed)
- [ ] `pnpm tauri dev` works with sidecar
- [ ] Download script fetches correct binary per platform
- [ ] Build size documented (FFmpeg static is ~70-100MB)
- [ ] All encoder tests pass with sidecar path

---

### Step 9: Format Versioning + Integrity 🟡 IMPORTANT

**Problem:** The .gameclip format has no version field and no integrity checks. As the format evolves, old clips may become unreadable. Corrupted clips fail silently.

**Solution:** Add format version to metadata, SHA-256 checksums for video/audio data, and migration support.

**Design:**
- Add `format_version: u32` to `ClipMetadata` (default 1 for existing clips via `#[serde(default)]`)
- Add `checksums: HashMap<String, String>` to metadata (e.g., `{"video.bin": "sha256:abc..."}`)
- Compute checksums during `write_clip()`, verify during `read_clip()`
- Version 1 = current format, version 2 = with checksums + pre-encoded video
- `read_clip()` checks version and applies migration if needed

**Files to modify:**
- `clip/metadata.rs` — add `format_version`, `checksums` fields
- `clip/format.rs` — compute/verify checksums, version check on read
- Migration functions for v1 → v2

**Checkpoint 9:**
- [ ] `format_version` field present in all new clips
- [ ] Old clips (no version) read correctly with default version 1
- [ ] Checksums computed on write, verified on read
- [ ] Corrupted clip detected and reported (not silently wrong)
- [ ] Version migration tested (v1 → v2 roundtrip)
- [ ] All existing clip tests still pass

---

### Step 10: HuggingFace Dataset Upload Pipeline 🟡 IMPORTANT

**Problem:** The HuggingFace export works locally but there's no way to contribute data to a shared dataset. The data flywheel doesn't start until users can easily share annotated clips.

**Solution:** CLI export command + optional upload to a public HuggingFace dataset repo. No accounts, no backend API — just `huggingface_hub` style direct upload.

**Design:**
- New Tauri command: `export_to_huggingface` — exports clips to HF format and optionally pushes to a dataset repo
- Privacy: strip user-identifiable metadata before upload (game window titles, OS username, file paths)
- Consent: explicit opt-in UI in Settings page with clear data usage explanation
- Upload: use HuggingFace HTTP API (no Python dependency) to push files to a dataset repo
- Quality gate: only upload clips with quality score > configurable threshold
- Batch: upload in background, show progress in UI

**Files to create/modify:**
- `annotation/upload.rs` — new module for HF HTTP API upload
- `annotation/privacy.rs` — new module for PII scrubbing
- `commands.rs` — add `export_to_huggingface` command
- `engine.rs` — add upload settings to `AppSettings`
- Frontend: Settings page upload toggle + export dialog

**Checkpoint 10:**
- [ ] Privacy scrubbing removes file paths, OS usernames, window titles
- [ ] Local export to HuggingFace format works (already done — verify with new pipeline)
- [ ] HTTP upload to HuggingFace dataset repo succeeds
- [ ] Quality gate filters clips below threshold
- [ ] Upload is async/background with progress reporting
- [ ] Opt-in consent flow in Settings UI
- [ ] Round-trip test: export → upload → download → verify data integrity

---

## Future Steps (deferred)

### Step 11: CI/CD Pipeline
GitHub Actions for Rust tests, clippy, TypeScript check, cross-compilation.

### Step 12: Frontend Tests + Quality Display
Vitest for React components, quality score badges in ClipCard, annotation viewer in ClipPlayer.

### Step 13: Windows Hardware Testing
Validate DXGI, Raw Input, WASAPI on real gaming PCs. Fix WASAPI int16 assumption. Anti-cheat compatibility testing.

### Step 14: Backend API
Node.js + Hono, PostgreSQL, Cloudflare R2. Clip upload/download, user profiles, dataset browsing.

### Step 15: Bounty Marketplace
Bounty CRUD, submissions, Stripe Connect payouts. Web + desktop bounty browser.

### Step 16: Web Clip Viewer
Shareable clip URLs, input overlay player, embeddable iframe, mobile responsive.

---

## Revenue Streams

1. **AI Training Datasets** (primary) — the first open collection of frame-action-annotated gameplay. Enterprise API access, curated dataset licensing, custom annotation for game studios.
2. **Bounty Marketplace** (secondary) — indie devs pay gamers to clip specific achievements. GameClip takes 10-15% platform fee.

---

## Current Status

| Step | Status | Tests |
|------|--------|-------|
| 1. Project Scaffolding | ✅ Complete | — |
| 2. Platform Abstraction | ✅ Complete | 28 tests |
| 3. Ring Buffer + Clip Save | ✅ Complete | 23 tests |
| 4. Clip Library UI | ✅ Complete | TS clean |
| 5. Data Annotation Pipeline | ✅ Complete | 51 tests |
| 6. Windows Capture Engine | 🔧 Partial (untested) | 6 tests |
| 7. In-Capture Encoding | ⏳ Not started | — |
| 8. Bundle FFmpeg Sidecar | ⏳ Not started | — |
| 9. Format Versioning | ⏳ Not started | — |
| 10. HuggingFace Upload | ⏳ Not started | — |

**Total: 128 passing Rust tests (1 flaky timing test), 0 frontend tests.**
