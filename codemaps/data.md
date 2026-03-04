# GameClip Data Models Codemap

> Freshness: 2026-02-26 | Path: apps/desktop/src-tauri/src/

## Serialized Types (cross Rust<->JSON<->TypeScript boundary)

### ClipMetadata (clip/metadata.rs)
```
id: String                         game: Option<String>
name: String                       width/height: u32
fps: u32                           duration_secs: f64
input_event_count: u64             has_audio: bool
audio_sample_rate: Option<u32>     audio_channels: Option<u16>
created_at: DateTime<Utc>          devices: CaptureDevices
video_encoded: bool (default=true) video_start_timestamp_us: u64 (default=0)
annotation_layers: Vec<String>     format_version: u32 (default=1)
checksums: HashMap<String,String>  (SHA-256 hex per zip entry)
```

### AppSettings (engine.rs)
```
buffer_duration_secs: u32 (30)     save_directory: String (~GameClip/clips)
hotkey: String (Ctrl+Shift+R)      capture_fps: u32 (30)
capture_width: u32 (1920)          capture_height: u32 (1080)
huggingface: HuggingFaceConfig     (#[serde(default)])
```

### HuggingFaceConfig (upload/hf_client.rs)
```
upload_consent: bool (false)       token: String (skip_serializing)
repo_id: String ("")               quality_gate: f64 (0.3)
private_repo: bool (false)
```

### InputEvent (input/mod.rs) — serde: flatten kind, tag="type"
```
timestamp_us: u64
kind: InputEventKind (flattened)
  Key        { key: String, pressed: bool }
  MouseButton { button: left|right|middle, pressed: bool, x/y: f64 }
  MouseMove   { x/y: f64 }
  MouseScroll { delta_x/delta_y: f64 }
```
JSON: `{"timestamp_us":12345,"type":"key","key":"KeyW","pressed":true}`

### FrameAction (annotation/types.rs)
```
frame: u64                         timestamp_us: u64
keys_held: Vec<String>             mouse_buttons_held: Vec<String>
mouse_x/y: f64                     mouse_dx/dy: f64
scroll_dx/dy: f64
```

### QualityScore (annotation/types.rs)
```
overall_score: f64 (0-1)           genre: String
dimension_scores: DimensionScores  action_density: f64
input_activity_ratio: f64          avg/peak_simultaneous_keys: f64/u32
avg/peak_mouse_speed: f64          unique_keys_used: u32
input_continuity: f64              mouse_control_smoothness: f64
highlights: Vec<HighlightSegment>  edge_case_flags: Vec<String>
```

### DimensionScores (annotation/types.rs)
```
action_density: f64                input_continuity: f64
input_diversity: f64               mouse_control: f64
action_complexity: f64             highlight_density: f64
```

### UploadProgress / UploadStage (upload/progress.rs)
```
UploadProgress:
  current_clip/total_clips: u32    clip_name: String
  stage: UploadStage               bytes_uploaded/total_bytes: u64

UploadStage: Preparing | UploadingVideo | UploadingMetadata | Committing | Done | Failed{reason}
```

### ClipSummary (commands.rs) — frontend-facing flat struct
```
id, name, game: Option<String>     duration_secs: f64
created_at: String                 file_path: String
input_event_count: u64             has_audio: bool
width/height: u32                  fps: u32
video_encoded: bool
```

## Runtime-Only Types (not serialized)

### EngineState (engine.rs)
```
saver: Arc<Mutex<ClipSaver>>       running: Arc<AtomicBool>
settings: Mutex<AppSettings>       upload_cancel: Mutex<Arc<AtomicBool>>
```

### CapturedFrame (capture/mod.rs)
```
timestamp_us: u64                  width/height: u32
data: Vec<u8> (w*h*4)             pixel_format: Rgba | Bgra
```

### AudioBuffer (audio/mod.rs)
```
timestamp_us: u64                  channels: u16
sample_rate: u32                   samples: Vec<f32> (interleaved PCM)
```

### EncodedChunk (sync/encoded_ring_buffer.rs)
```
timestamp_us: u64                  data: Vec<u8> (MP4 box bytes)
chunk_type: InitSegment | MediaSegment
```

### RingBuffer<T: Timestamped> (sync/ring_buffer.rs)
```
items: VecDeque<T>                 max_duration_us: u64
```

### EncodedRingBuffer (sync/encoded_ring_buffer.rs)
```
init_segment: Option<Vec<u8>>      chunks: VecDeque<EncodedChunk>
max_duration_us: u64               fragment_duration_us: u64
first_raw_frame: Option<CapturedFrame>
```

### SyncClock (sync/clock.rs)
```
epoch: Arc<Instant>                (Clone shares same epoch)
now_us() -> u64
```

## .gameclip Zip Format

| Entry | Required | Format | Written |
|---|---|---|---|
| metadata.json | yes | Pretty JSON (ClipMetadata) | LAST (checksums embedded) |
| input.jsonl | yes | NDJSON (InputEvent per line) | 1st |
| video.bin | yes | fMP4 (H.264) or raw RGBA | 2nd |
| audio.bin | no | Raw PCM f32 LE | 3rd |
| thumbnail.jpg | no | JPEG | 4th |
| frame_actions.jsonl | no | NDJSON (FrameAction per line) | 5th |
| quality.json | no | Pretty JSON (QualityScore) | 6th |

**Format versions:** v1 = no checksums (legacy), v2 = SHA-256 per entry in metadata.checksums

## Serialization Boundary Summary

**Serde Serialize+Deserialize:** ClipMetadata, CaptureDevices, InputEvent/Kind/*, FrameAction, QualityScore, DimensionScores, HighlightSegment, ClipAnnotations, AnnotationManifest, DatasetStats, AppSettings, HuggingFaceConfig, UploadProgress, UploadStage, ExportResult, GameGenre

**Custom Serialize (as string):** HfError

**Skip-serializing fields:** HuggingFaceConfig.token

**Key serde attributes:**
- `InputEventKind`: `#[serde(tag = "type")]` + `#[serde(flatten)]` on InputEvent.kind
- `MouseButton`: `#[serde(rename_all = "lowercase")]`
- `ClipMetadata`: multiple `#[serde(default)]` and `#[serde(default = "fn")]` for backward compat
- `QualityScore`: `#[serde(default)]` on genre, dimension_scores, continuity, smoothness

## Configuration

### tauri.conf.json
```
productName: GameClip, version: 0.1.0, identifier: com.gameclip.app
devUrl: http://localhost:1420
window: 1024x720 (min 800x600)
assetProtocol: enabled, scope: ["**"]
externalBin: ["binaries/ffmpeg"]   (FFmpeg sidecar)
```

### Cargo.toml workspace
```
resolver = "2", members = ["apps/desktop/src-tauri"]
```

### Key Cargo dependencies
tauri 2, serde 1, serde_json 1, chrono 0.4, uuid 1, thiserror 2, zip 2, image 0.25, sha2 0.10, hex 0.4, reqwest 0.12, base64 0.22, sysinfo 0.38, log 0.4, simplelog 0.12

### Frontend dependencies
react ^19.1.0, react-dom ^19.1.0, @tauri-apps/api ^2, vite ^7.0.4, vitest ^4.0.18, typescript ~5.8.3
