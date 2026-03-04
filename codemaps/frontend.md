# GameClip Frontend Codemap

> Freshness: 2026-02-26 | 3 Vitest tests | Path: apps/desktop/src/

## File Map

```
src/
  main.tsx              — ReactDOM.createRoot()
  App.tsx               — Page router (useState<Page>), nav bar, hook wiring
  vite-env.d.ts         — Vite client types
  test-setup.ts         — Vitest config: mocks @tauri-apps/api/{core,event}
  pages/
    ClipLibrary.tsx     — Grid of ClipCards, save/delete actions
    ClipPlayer.tsx      — rAF video playback + InputOverlay
    Settings.tsx        — Draft-state form (capture, storage, HuggingFace config)
  components/
    ClipCard.tsx        — Thumbnail card with delete button (memo'd)
    InputOverlay.tsx    — Real-time key/mouse/click overlay synced to playback
  hooks/
    useClips.ts         — Clip CRUD + "clip-saved" event listener
    useClipData.ts      — Video extraction + input event loading
    useThumbnail.ts     — Base64 JPEG thumbnail with module-level cache
    useSettings.ts      — AppSettings get/update
    useUpload.ts        — Upload IPC + "upload-progress" listener
    useUpload.test.ts   — 3 tests (mock Tauri invoke/listen)
```

## Component Tree & Props

```
App (root — no props)
  nav bar (inline JSX: Library | Settings buttons)
  ClipLibrary
    clips: ClipSummary[], loading, error, onSelectClip, onDeleteClip, onSaveClip
    -> ClipCard[] (memo'd)
         clip: ClipSummary, onSelect(clip), onDelete(filePath)
         -> useThumbnail(clip.file_path)
  ClipPlayer
    clip: ClipSummary, onBack()
    -> useClipData(clip.file_path)
    -> InputOverlay
         events: InputEvent[], currentTimeUs, width, height,
         captureWidth? (1920), captureHeight? (1080)
  SettingsPage
    settings: AppSettings|null, loading, error, onUpdate(settings), onBack()
```

## Page Routing

Manual state-machine in `App.tsx`:
```
Page = "library" | "player" | "settings"
[page, setPage] = useState<Page>("library")
[selectedClip, setSelectedClip] = useState<ClipSummary | null>(null)
```
- library -> player: `handleSelectClip(clip)`
- player -> library: `handleBack()` (clears selectedClip)
- any -> settings: nav button
- settings -> library: `handleBack()`

Pages unmount when inactive (conditional render, not React Router).

## Hook -> IPC Mapping

| Hook | Tauri invoke() | Tauri listen() | State |
|---|---|---|---|
| useClips | list_clips, delete_clip, save_clip | "clip-saved" | clips[], loading, error |
| useClipData | extract_clip_video, get_clip_input_events | — | videoUrl, inputEvents[], loading, error |
| useThumbnail | get_clip_thumbnail | — | thumbnail (string) |
| useSettings | get_settings, update_settings | — | settings, loading, error |
| useUpload | upload_clips, cancel_upload | "upload-progress" | uploading, progress, error |

All hooks guard IPC with `isTauri()` check for graceful degradation outside Tauri.

## Data Flow

```
Rust backend
  list_clips          -> useClips.clips[]      -> ClipLibrary -> ClipCard[]
  save_clip           -> useClips.saveClip()   -> Library "Save Clip" btn
  delete_clip         -> useClips.deleteClip() -> ClipCard delete btn
  get_clip_thumbnail  -> useThumbnail.thumbnail -> ClipCard img src
  extract_clip_video  -> useClipData.videoUrl  -> ClipPlayer <video src>
  get_clip_input_events -> useClipData.inputEvents -> InputOverlay
  get/update_settings -> useSettings           -> SettingsPage form
  upload_clips        -> useUpload             -> (not yet wired to UI)
  "clip-saved" event  -> useClips.fetchClips() -> refresh list
  "upload-progress"   -> useUpload.progress    -> (not yet wired to UI)
  convertFileSrc()    -> browser asset:// URL  -> <video> src
```

## InputOverlay Internals

Three `useMemo` derivations from `(events, currentTimeUs)`:
1. `visibleEvents` — filter to 500ms window around currentTimeUs
2. `activeKeys` — walk all events up to currentTimeUs, accumulate key state machine
3. `mousePosition` + `mouseClicks` — latest position, clicks within 300ms

## TypeScript Types

| Type | File | Exported | Description |
|---|---|---|---|
| ClipSummary | useClips.ts | yes | id, name, game, duration_secs, file_path, etc. |
| ClipInputEvent | useClipData.ts | yes | timestamp_us, type, key?, pressed?, button?, x?, y?, delta_x?, delta_y? |
| AppSettings | useSettings.ts | yes | buffer_duration_secs, save_directory, hotkey, capture_fps/width/height, huggingface |
| HuggingFaceConfig | useSettings.ts | yes | upload_consent, token, repo_id, quality_gate, private_repo |
| UploadProgress | useUpload.ts | yes | current_clip, total_clips, clip_name, stage, bytes_uploaded, total_bytes |
| UploadStage | useUpload.ts | yes | "Preparing" \| "UploadingVideo" \| ... \| { Failed: { reason } } |
| Page | App.tsx | no | "library" \| "player" \| "settings" |

## Dependencies

| Package | Version | Usage |
|---|---|---|
| react | ^19.1.0 | useState, useCallback, useEffect, useRef, useMemo, memo |
| react-dom | ^19.1.0 | createRoot in main.tsx |
| @tauri-apps/api/core | ^2 | invoke, isTauri, convertFileSrc |
| @tauri-apps/api/event | ^2 | listen, UnlistenFn |

No third-party UI library. All styling is custom CSS class names.

## Architectural Notes

1. **Module-level thumbnail cache** — `useThumbnail` uses `Map<string, string|null>` outside React, survives remounts
2. **rAF playback sync** — ClipPlayer uses `requestAnimationFrame` (not `timeupdate`) for sub-frame overlay precision
3. **ClipCard memoization** — `memo()` prevents re-renders since each card fetches its own thumbnail
4. **Draft-state settings** — SettingsPage copies settings into draft state, only persists on explicit save
5. **useUpload is orphaned** — Fully implemented but not imported in any page component yet
