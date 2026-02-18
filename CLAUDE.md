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
| TypeScript check | `cd apps/desktop && npx tsc --noEmit` |
| Cross-compile Windows | `cargo xwin build --target x86_64-pc-windows-msvc --release` |

Vite dev server runs on port 1420 (hardcoded in `vite.config.ts`).

## Architecture

Tauri v2 monorepo: Rust backend captures screen/input/audio, React+TypeScript frontend displays clips. Developed on Mac with mock implementations; real capture targets Windows only.

### Rust Backend (`apps/desktop/src-tauri/src/`)

**Platform abstraction pattern** — Each capture subsystem has a trait + mock + windows impl:
- `capture/mod.rs` defines `ScreenCapture` trait → `capture/mock.rs` (color-cycling RGBA frames)
- `input/mod.rs` defines `InputRecorder` trait → `input/mock.rs` (WASD/mouse events)
- `audio/mod.rs` defines `AudioCapture` trait → `audio/mock.rs` (440Hz sine wave)
- Windows impls are `#[cfg(target_os = "windows")]` gated (stubs, not yet implemented)

**Data flow:** Capture sources → `RingBuffer<T: Timestamped>` (last N seconds) → `ClipSaver` drains buffers → `write_clip()` produces `.gameclip` zip archive.

**Key modules:**
- `engine.rs` — `EngineState` (managed Tauri state): owns `ClipSaver`, `AppSettings`, spawns capture threads. Currently MVP: `save_clip()` generates fresh mock data instead of flushing the ring buffer.
- `commands.rs` — Six `#[tauri::command]` IPC functions: `list_clips`, `get_clip_metadata`, `delete_clip`, `save_clip`, `get_settings`, `update_settings`
- `lib.rs` — App entry: registers state, system tray, global shortcut (`Ctrl+Shift+R`), IPC handlers. On hotkey: saves clip and emits `"clip-saved"` event to webview.
- `sync/ring_buffer.rs` — Generic `RingBuffer<T>` backed by `VecDeque`, evicts by `max_duration_us`
- `sync/clock.rs` — `SyncClock` wraps `std::time::Instant`, provides monotonic microsecond timestamps
- `clip/format.rs` — `.gameclip` is a zip containing `metadata.json`, `input.jsonl`, `video.bin`, optional `audio.bin` and `thumbnail.jpg`
- `clip/saver.rs` — `ClipSaver` holds three ring buffers, assembles clip data, generates metadata
- `clip/metadata.rs` — `ClipMetadata` serde struct with `CaptureDevices`

### Frontend (`apps/desktop/src/`)

Simple page router in `App.tsx` using `useState<"library" | "player" | "settings">`.

**Hooks (Tauri IPC bridge):**
- `useClips` — calls `invoke()` for clip CRUD, listens for `"clip-saved"` events from Rust
- `useSettings` — calls `invoke()` for settings get/update

**Pages:** `ClipLibrary` (grid of ClipCards), `ClipPlayer` (rAF-based playback with InputOverlay), `Settings` (form with draft state pattern).

**`InputOverlay`** — Renders active keys, mouse cursor, and click ripples synced to playback time using 500ms visibility windows.

### `.gameclip` Format

Zip archive containing:
- `metadata.json` — clip id, game, resolution, fps, duration, device flags, timestamps
- `input.jsonl` — one `InputEvent` per line (tagged union: Key, MouseButton, MouseMove, MouseScroll)
- `video.bin` — raw RGBA (mock) / H.264 (future Windows)
- `audio.bin` — raw PCM f32 LE (optional)
- `thumbnail.jpg` — first frame data (optional)

## Known MVP Limitations

- `engine.rs` capture thread populates `mpsc` channels but the drain thread discards data. `save_clip()` creates fresh mock sources instead of flushing ring buffers. Production fix: capture thread pushes directly into `Arc<Mutex<ClipSaver>>`.
- Thumbnails are raw bytes, not actual JPEG images.
- Frontend `ClipPlayer` generates its own mock input events client-side rather than reading from the clip file.
- `SyncClock` instances per stream are independent (not shared), so cross-stream sync is not guaranteed.

## Testing

51 Rust unit tests, all co-located with source (`#[cfg(test)] mod tests`). No frontend tests exist.

Tests use `tempfile::TempDir` for filesystem operations and `std::thread::sleep` for timing. Pattern: test state machine transitions (start/stop), error conditions (double-start, poll-without-start), data integrity, and serialization round-trips.

## Implementation Status

Steps 1-4 complete (scaffolding, platform abstraction, ring buffer + clip save, clip library UI). Next: Step 5 (Windows capture engine). See `PLAN.md` for full roadmap.
