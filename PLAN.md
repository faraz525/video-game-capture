# GameClip — Implementation Plan

## Context

Build a desktop screen capture tool for gamers (Windows target, developed on Mac Apple Silicon) that records gameplay clips with synchronized keyboard, mouse, and audio data. The clip tool is the distribution vehicle for two revenue streams: a bounty marketplace (indie devs pay community to clip achievements) and AI training datasets (gameplay + input data sold to model providers).

**Key decisions:** Windows-only, open-source capture engine, bounty marketplace first, KB/mouse + game audio.

---

## Project Structure

```
gameclip/
├── apps/
│   ├── desktop/               # Tauri app (Rust backend + web frontend)
│   │   ├── src-tauri/         # Rust: capture engine, system tray, hotkeys
│   │   │   ├── src/
│   │   │   │   ├── capture/   # Platform-abstracted capture (traits + impls)
│   │   │   │   ├── input/     # Input recording (traits + impls)
│   │   │   │   ├── audio/     # Audio capture (traits + impls)
│   │   │   │   ├── sync/      # Sync clock + ring buffer
│   │   │   │   ├── clip/      # Clip packaging (.gameclip format)
│   │   │   │   └── game/      # Game detection
│   │   │   └── Cargo.toml
│   │   └── src/               # Frontend: React + TypeScript
│   │       ├── components/    # UI components
│   │       ├── pages/         # Clip library, settings, bounty browser
│   │       └── hooks/         # Tauri IPC hooks
│   └── web/                   # Web app (clip viewer, bounty marketplace)
├── packages/
│   ├── clip-format/           # Shared .gameclip format lib (Rust)
│   └── shared-types/          # Shared TypeScript types
├── services/
│   └── api/                   # Backend API (Node.js + Hono)
├── Cargo.toml                 # Workspace root
├── package.json               # pnpm workspace root
└── turbo.json                 # Turborepo config
```

---

## Implementation Steps

### Step 1: Project Scaffolding ✅
**Goal:** Tauri app boots, shows a window, sits in system tray.

- Init git repo at `/Users/faraz525/Documents/gameclip`
- Init Rust workspace (`Cargo.toml`) with `apps/desktop/src-tauri` member
- Scaffold Tauri v2 app with React + TypeScript frontend (Vite)
- Configure system tray with basic menu (Settings, Quit)
- Register global hotkey (Ctrl+Shift+R) that logs to console
- Add pnpm workspace for JS packages

**Checkpoint 1:**
- [x] `cargo build` succeeds
- [x] System tray icon appears with menu
- [x] Global hotkey press logs a message
- [x] Git repo initialized with first commit

---

### Step 2: Platform Abstraction Layer ✅
**Goal:** Define traits for all OS-specific operations with mock implementations that work on Mac.

Create Rust traits:
- `ScreenCapture` — start/stop capture, get frames
- `InputRecorder` — start/stop recording, poll input events
- `AudioCapture` — start/stop loopback, get audio buffers
- `SyncClock` — high-resolution timestamps

Implement `MockCapture` (generates colored frames at 60fps), `MockInput` (generates random key/mouse events), `MockAudio` (generates silence/sine wave).

Use `cfg(target_os)` to compile real impls on Windows, mocks on Mac.

**Files:**
- `src-tauri/src/capture/mod.rs` — traits
- `src-tauri/src/capture/mock.rs` — mock implementations
- `src-tauri/src/capture/windows.rs` — stubs (compile-gated)
- `src-tauri/src/input/mod.rs`, `mock.rs`, `windows.rs`
- `src-tauri/src/audio/mod.rs`, `mock.rs`, `windows.rs`
- `src-tauri/src/sync/clock.rs` — cross-platform clock abstraction

**Checkpoint 2:**
- [x] All traits defined with comprehensive doc comments
- [x] Mock implementations pass unit tests
- [x] `MockCapture` generates 60fps synthetic frames
- [x] `MockInput` generates timestamped input events
- [x] `SyncClock` produces monotonic microsecond timestamps
- [x] `cargo test` passes on Mac (51 tests)

---

### Step 3: Ring Buffer + Clip Save ✅
**Goal:** Continuously buffer mock frames/input/audio, save to disk on hotkey.

Implement ring buffer that holds last N seconds of:
- Video frames (as raw/compressed buffers)
- Input events (append-only log)
- Audio buffers

On hotkey press: flush ring buffer to `.gameclip` package.

**Clip package format:**
```
clip_001.gameclip/  (zip archive)
├── video.bin           # Video data (H.264 on Windows, raw frames for mock)
├── input.jsonl         # Timestamped input events
├── metadata.json       # Game, resolution, fps, duration, devices
└── thumbnail.jpg       # First frame as thumbnail
```

**Files:**
- `src-tauri/src/sync/ring_buffer.rs` — generic ring buffer
- `src-tauri/src/clip/mod.rs` — clip packaging logic
- `src-tauri/src/clip/format.rs` — .gameclip read/write
- `src-tauri/src/clip/metadata.rs` — metadata schema
- `src-tauri/src/clip/saver.rs` — coordinates ring buffers and saves
- `src-tauri/src/commands.rs` — Tauri IPC commands
- `src-tauri/src/engine.rs` — capture engine state management

**Checkpoint 3:**
- [x] Ring buffer correctly manages N seconds of data (unit tests)
- [x] Ring buffer overwrites oldest data when full
- [x] Hotkey triggers clip save from ring buffer
- [x] `.gameclip` file is a valid zip with correct structure
- [x] `input.jsonl` contains properly timestamped events
- [x] `metadata.json` matches schema and has correct values
- [x] Clip can be saved and re-read programmatically (round-trip test)

---

### Step 4: Clip Library UI ✅
**Goal:** User can view saved clips, see input overlay, manage clips.

Built React frontend pages:
- **Clip Library** — grid of saved clips with thumbnails
- **Clip Player** — video playback with synchronized input overlay (shows keypresses, mouse position/clicks as they happened)
- **Settings** — hotkey config, buffer duration, save location

Tauri IPC commands:
- `list_clips()` → returns clip metadata
- `get_clip_metadata(file_path)` → returns full metadata
- `delete_clip(file_path)` → removes clip
- `save_clip()` → triggers clip save
- `get_settings()` / `update_settings()`

**Checkpoint 4:**
- [x] Clip library shows saved clips in grid
- [x] Clicking a clip opens the player
- [x] Input overlay renders keyboard presses and mouse movements on top of video
- [x] Input events visually sync with video playback
- [x] Settings page allows changing hotkey and buffer duration
- [x] Frontend TypeScript compiles clean
- [x] Vite build succeeds

---

### Step 5: Windows Capture Engine (requires Windows VM)
**Goal:** Replace mock implementations with real Windows APIs.

Implement:
- `WindowsCapture` — DXGI Desktop Duplication API for screen capture
- `WindowsEncoder` — NVENC (NVIDIA) / AMF (AMD) hardware encoding to H.264
- `WindowsInput` — Raw Input API (`WM_INPUT` + `RIDEV_INPUTSINK`) for KB/mouse
- `WindowsAudio` — WASAPI loopback for game audio
- `WindowsClock` — `QueryPerformanceCounter` for high-res timestamps
- `GameDetector` — enumerate running processes, match against known game executables

**Files:**
- `src-tauri/src/capture/windows.rs` — DXGI + NVENC/AMF
- `src-tauri/src/input/windows.rs` — Raw Input API
- `src-tauri/src/audio/windows.rs` — WASAPI loopback
- `src-tauri/src/sync/clock_windows.rs` — QPC wrapper
- `src-tauri/src/game/detector.rs` — process enumeration

**Dev workflow:**
1. Write Rust code on Mac (compiles but can't run Windows APIs)
2. Cross-compile: `cargo xwin build --target x86_64-pc-windows-msvc`
3. Transfer binary to AWS g4dn.xlarge Windows VM
4. Test with a real game, verify capture quality

**Checkpoint 5:**
- [ ] Cross-compilation succeeds from Mac to Windows
- [ ] App launches on Windows VM
- [ ] DXGI captures game frames at native resolution
- [ ] Hardware encoder produces valid H.264 MP4
- [ ] Raw Input captures KB/mouse even when game is focused
- [ ] WASAPI captures game audio
- [ ] All streams are synced within <1ms (verify with input overlay)
- [ ] FPS impact is <3 frames at 1080p60
- [ ] Game detector correctly identifies the running game
- [ ] Full clip save produces valid `.gameclip` with real data

---

### Step 6: Backend API
**Goal:** Users can upload clips, create accounts, browse community clips.

- Node.js + Hono API server
- PostgreSQL database (users, clips, bounties)
- Cloudflare R2 for clip storage (S3-compatible, no egress fees)
- Auth: Discord + Steam OAuth
- Endpoints: clip CRUD, user profile, clip upload/download

**Files:**
- `services/api/src/index.ts` — Hono app entry
- `services/api/src/routes/clips.ts` — clip endpoints
- `services/api/src/routes/auth.ts` — OAuth flows
- `services/api/src/routes/users.ts` — user profiles
- `services/api/src/db/schema.ts` — Drizzle ORM schema
- `services/api/src/storage/r2.ts` — R2 upload/download

**Checkpoint 6:**
- [ ] API server starts and responds to health check
- [ ] Discord OAuth login flow works end-to-end
- [ ] Clip metadata can be created, read, updated, deleted
- [ ] Clip file uploads to R2 and downloads correctly
- [ ] User profiles store and retrieve correctly
- [ ] Input validation rejects malformed requests (Zod schemas)
- [ ] All endpoints have integration tests passing

---

### Step 7: Bounty Marketplace MVP
**Goal:** Game devs can post bounties, gamers can submit clips to fulfill them.

- Bounty CRUD (title, game, requirements, reward amount, deadline)
- Bounty browser in desktop app and web
- Clip submission to bounty (select existing clip or record new one)
- Review workflow (bounty poster approves/rejects submissions)
- Stripe Connect for payouts

**Files:**
- `services/api/src/routes/bounties.ts` — bounty endpoints
- `services/api/src/routes/submissions.ts` — submission workflow
- `services/api/src/payments/stripe.ts` — Stripe Connect integration
- `apps/web/src/pages/bounties/` — web bounty browser
- `apps/desktop/src/pages/bounties/` — in-app bounty browser

**Checkpoint 7:**
- [ ] Game dev can create a bounty with requirements and reward
- [ ] Gamers can browse bounties filtered by game
- [ ] Gamer can submit a clip to a bounty
- [ ] Bounty poster can review and approve/reject submissions
- [ ] Approved submission triggers Stripe payout to gamer
- [ ] Rejected submission shows rejection reason
- [ ] End-to-end flow: post bounty → submit clip → approve → payout

---

### Step 8: Clip Replay Viewer (Web)
**Goal:** Shareable web-based clip viewer with input overlay.

- Web player that renders video with synchronized input overlay
- Public share links for clips
- Embeddable player (iframe) for game dev marketing
- Input visualization: keyboard heatmap, mouse trail, click indicators

**Checkpoint 8:**
- [ ] Public clip URL renders video with input overlay
- [ ] Input overlay shows keypresses, mouse movement, and clicks in sync
- [ ] Player works on mobile (responsive)
- [ ] Embed code generates working iframe
- [ ] Loading time is <3 seconds for a 30s clip

---

## Dev Environment Setup (Mac → Windows)

**Local (Mac):** Develop UI, backend, business logic, Rust with mocks
**Remote (AWS g4dn.xlarge):** Test Windows capture engine with real games

```
Mac dev cycle:
  code → cargo test → pnpm tauri dev (with mocks) → iterate

Windows test cycle:
  cargo xwin build → git push → pull on VM → run .exe → test with game
```

**Cross-compilation setup:**
```bash
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin
cargo xwin build --target x86_64-pc-windows-msvc --release
```

---

## AWS g4dn.xlarge Cost Estimate

The g4dn.xlarge provides an NVIDIA T4 GPU for testing the capture engine with real games.

| Option | Approx. Cost | Notes |
|--------|-------------|-------|
| On-demand | ~$0.71/hr | Start/stop as needed, pay per hour |
| Spot instance | ~$0.21/hr | 70% cheaper, can be interrupted |
| Reserved (1yr) | ~$0.45/hr | Commit for steady use |

**Recommended approach:** Use spot instances during development. A typical test session (2-3 hours) costs ~$0.50-0.65. Budget ~$20-30/month for periodic testing.

---

## Revenue Streams

1. **Bounty Marketplace** (Step 7) — indie devs pay gamers to clip specific achievements/moments. GameClip takes a platform fee (10-15%).
2. **AI Training Datasets** — gameplay video + synchronized input data sold to model providers training game-playing AI. The input overlay data is the key differentiator vs plain screen recordings.

---

## Current Status

| Step | Status | Tests |
|------|--------|-------|
| 1. Project Scaffolding | ✅ Complete | — |
| 2. Platform Abstraction | ✅ Complete | 28 tests |
| 3. Ring Buffer + Clip Save | ✅ Complete | 23 tests |
| 4. Clip Library UI | ✅ Complete | TS clean |
| 5. Windows Capture Engine | 🔧 Code written (untested on Windows) | 6 tests |
| 6. Backend API | Not started | — |
| 7. Bounty Marketplace | Not started | — |
| 8. Web Clip Viewer | Not started | — |

**Total: 57 passing Rust tests, 0 failures. Clippy clean.**
