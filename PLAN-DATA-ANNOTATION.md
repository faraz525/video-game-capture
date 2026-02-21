# Data Annotation Platform — Plan

## Why This Play Has Value

### The Market Signal

The evidence that action-conditioned gaming data is extraordinarily valuable is overwhelming:

- **OpenAI offered $500M** to acquire Medal.tv for its gaming video dataset. Medal declined.
- **General Intuition** (Medal.tv spinoff) raised **$134M seed** from Khosla Ventures and General Catalyst to train world models on gaming clips. Their thesis: "Games are basically the only verifiable domain for spatial-temporal reasoning."
- **$1.3B+** flowed into world model startups in early 2026 alone (AMI Labs $587M, World Labs $230M, Luma AI $900M, Runway $315M).
- The **AI training dataset market** is projected to grow from $3.6B (2025) to **$23.2B by 2034** (22.9% CAGR).

### Why GameClip Is Uniquely Positioned

The `.gameclip` format already captures the exact data structure that world models train on:

1. **Synchronized video + input events** — the `(action, observation)` pair structure world models need
2. **Per-event microsecond timestamps** — via shared `SyncClock` epoch across all streams
3. **Multi-modal** — video + keyboard + mouse + audio in one package
4. **Game detection** — automatic game labeling via process detection
5. **Structured format** — `input.jsonl` with tagged union `InputEvent` types maps closely to research formats like D2E/OWAMcap

### The Gap: Raw Capture → ML-Ready Dataset

Raw `.gameclip` data is valuable, but not directly consumable by ML researchers. They need:

| Layer | What Researchers Need | What GameClip Has Today |
|---|---|---|
| Frame-indexed actions | Per-frame discrete action buckets | Event-stream only (press/release timestamps) |
| Game state | Health, score, position, inventory | Game name only |
| Scene understanding | Object masks, depth maps, scene descriptions | Raw pixels only |
| Text captions | Natural language descriptions at 1-5s intervals | None |
| Quality signals | Edge case flags, highlight scores, episode boundaries | None |
| Export formats | HuggingFace Datasets, MCAP, MP4+JSON sidecar | `.gameclip` zip only |

**The play:** Build annotation pipelines that transform raw `.gameclip` data into richly annotated training datasets, exportable in formats researchers already use. This is the difference between selling raw ore and selling refined metal.

### Competitive Landscape

| Player | Data Source | Annotation | Scale |
|---|---|---|---|
| **General Intuition** | Medal.tv (2B clips/yr) | Proprietary pipeline | 10M users |
| **D2E / Open World Agents** | Custom recorder (29 games) | Nanosecond-precision MCAP | 273 hours |
| **Hunyuan-GameCraft** | In-house capture (100+ AAA) | Auto-annotated captions | 1M+ clips |
| **GameFactory** | Minecraft contractor play | Per-frame JSON actions | ~2000 frames/clip |
| **GameClip** (proposed) | User-generated clips | Multi-layer auto-annotation | Starting small, scaling with users |

GameClip's advantage: users generate clips voluntarily (free data acquisition), across many games, with ground-truth input labels from the capture engine itself. No RL agent, no contractors, no screen recording software — native capture with native input hooks.

### Revenue Model

**Tiered dataset sales to world model companies:**

| Tier | Contents | Target Price |
|---|---|---|
| Raw | Video + input events + metadata | $0.50–$2/clip-hour |
| Annotated | + frame-indexed actions, game state, captions | $5–$20/clip-hour |
| Premium | + scene segmentation, depth maps, physics labels | $20–$100/clip-hour |

For context: NVIDIA Cosmos trained on 20M hours of video. Even at $1/hour, that's a $20M dataset. Annotated gaming data with ground-truth input labels commands a significant premium over generic video.

---

## What the Annotation Platform Looks Like

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     .gameclip (existing)                     │
│  ┌──────────┐  ┌───────────┐  ┌─────────┐  ┌────────────┐  │
│  │video.bin │  │input.jsonl│  │audio.bin│  │metadata.json│  │
│  └──────────┘  └───────────┘  └─────────┘  └────────────┘  │
└──────────────────────┬──────────────────────────────────────┘
                       │
         ┌─────────────▼─────────────┐
         │   Annotation Pipeline     │
         │                           │
         │  1. Frame Indexer         │  Convert event-stream → per-frame actions
         │  2. HUD Extractor         │  OCR/CV on game HUD elements
         │  3. Scene Annotator       │  Object detection, depth estimation
         │  4. Caption Generator     │  LLM-based dense text descriptions
         │  5. Quality Scorer        │  Highlight detection, edge case flags
         │  6. Episode Segmenter     │  Round/life/objective boundaries
         │                           │
         └─────────────┬─────────────┘
                       │
         ┌─────────────▼─────────────┐
         │  Annotated .gameclip      │
         │                           │
         │  + annotations.jsonl      │  Per-frame annotation records
         │  + scene_graph.jsonl      │  Object detections per frame
         │  + captions.jsonl         │  Text descriptions at intervals
         │  + quality.json           │  Clip-level quality metadata
         │  + metadata.json          │  Extended with annotation flags
         │                           │
         └─────────────┬─────────────┘
                       │
         ┌─────────────▼─────────────┐
         │   Export Adapters          │
         │                           │
         │  • HuggingFace Datasets   │  Arrow/Parquet for PyTorch DataLoader
         │  • MCAP                   │  For robotics/embodied AI researchers
         │  • MP4 + JSON sidecar     │  Cosmos/Hunyuan-GameCraft compatible
         │  • WebDataset             │  For large-scale distributed training
         │                           │
         └───────────────────────────┘
```

### Annotation Layers in Detail

#### Layer 1: Frame-Indexed Action Summary (highest priority)

Convert the event-stream `input.jsonl` into per-frame discrete action labels — the format every world model training pipeline expects.

**Input:** `input.jsonl` with microsecond-timestamped press/release/move events
**Output:** `frame_actions.jsonl` — one JSON object per video frame

```jsonl
{"frame": 0, "t_us": 0, "keys_held": ["KeyW", "ShiftLeft"], "mouse_buttons": ["Left"], "mouse_dx": 12.5, "mouse_dy": -3.2, "mouse_x": 540, "mouse_y": 320, "scroll_dy": 0.0}
{"frame": 1, "t_us": 16667, "keys_held": ["KeyW", "ShiftLeft"], "mouse_buttons": [], "mouse_dx": 8.1, "mouse_dy": 1.0, "mouse_x": 548, "mouse_y": 321, "scroll_dy": 0.0}
{"frame": 2, "t_us": 33334, "keys_held": ["KeyW"], "mouse_buttons": [], "mouse_dx": 0.0, "mouse_dy": 0.0, "mouse_x": 548, "mouse_y": 321, "scroll_dy": 0.0}
```

**Why this matters:** This is the single most requested data format across all world model papers. GF-Minecraft uses per-frame JSON with `ws`/`ad`/`scs` buckets. D2E records event streams but researchers convert to per-frame for training. DIAMOND and GameNGen both need frame-aligned actions. This conversion is purely algorithmic — no ML models needed.

**Implementation:** Walk the event stream maintaining a state machine of held keys and mouse buttons. For each video frame timestamp, snapshot the current state. Accumulate mouse deltas between frames.

#### Layer 2: Game State Extraction (high priority)

Extract structured game state from HUD elements using OCR and template matching.

**Output:** `game_state.jsonl` — one record per frame (or per second for efficiency)

```jsonl
{"frame": 0, "health": 85, "ammo": "24/120", "score": 1250, "minimap_hash": "a3f2...", "hud_elements": [{"type": "health_bar", "bbox": [10, 680, 200, 700], "value": 0.85}]}
```

**Approach:**
- Game-specific HUD templates for popular games (CS2, Valorant, Fortnite, etc.)
- Generic OCR fallback for unknown games (numbers, text on screen)
- Pre-trained game HUD detection model (fine-tuned YOLO or similar)
- Run as a post-processing step, not at capture time

#### Layer 3: Dense Text Captions (high priority)

Natural language descriptions of gameplay at configurable intervals.

**Output:** `captions.jsonl`

```jsonl
{"start_us": 0, "end_us": 3000000, "caption": "Player sprints forward through a corridor, ADS with rifle, spots enemy behind cover on the left"}
{"start_us": 3000000, "end_us": 6000000, "caption": "Player fires burst at enemy, lands 2 hits, enemy retreats behind wall, player reloads"}
```

**Approach:**
- Sample keyframes at 1-3 second intervals
- Run through vision-language model (GPT-4V, Claude, or open-source like LLaVA)
- Prompt: "Describe the gameplay action in this sequence of frames. Focus on what the player is doing, what is happening in the environment, and any significant events."
- Can be batched and run offline

**Why this matters:** UniSim and recent research shows dense text captions can serve as a scalable substitute for action labels, enabling training on video-only data.

#### Layer 4: Scene Understanding (medium priority)

Object detection, semantic segmentation, and depth estimation per frame.

**Output:** `scene_graph.jsonl`

```jsonl
{"frame": 0, "objects": [{"class": "player", "bbox": [120, 200, 180, 400], "confidence": 0.92}, {"class": "weapon", "bbox": [300, 350, 380, 400], "confidence": 0.88}], "depth_map_ref": "depth/frame_0000.png"}
```

**Approach:**
- Pre-trained models: YOLO for detection, SAM for segmentation, MiDaS/ZoeDepth for depth
- Game-specific fine-tuning for common games
- Run as batch process on extracted video frames
- Store depth maps as 16-bit PNGs in a `depth/` directory within the zip

#### Layer 5: Quality and Interest Scoring (medium priority)

Clip-level and segment-level quality metadata.

**Output:** `quality.json` (clip-level) + quality fields in `frame_actions.jsonl`

```json
{
  "overall_score": 0.85,
  "highlight_segments": [
    {"start_us": 5000000, "end_us": 8000000, "type": "multi_kill", "confidence": 0.9},
    {"start_us": 12000000, "end_us": 14000000, "type": "clutch_play", "confidence": 0.7}
  ],
  "edge_case_flags": ["rapid_camera_movement", "explosion_with_particles", "close_quarters_combat"],
  "action_density": 0.73,
  "visual_complexity": 0.65,
  "input_intensity": 0.81
}
```

**Why this matters:** World model researchers specifically want edge cases — moments where physics, occlusion, and fast dynamics stress-test the model. Gamers who clip and share gameplay already select for these moments. Quality scoring surfaces the most valuable training examples.

#### Layer 6: Episode Segmentation (lower priority)

Detect game round boundaries, death/respawn events, loading screens, and menu transitions.

**Output:** `episodes.jsonl`

```jsonl
{"type": "gameplay", "start_us": 0, "end_us": 45000000, "game_mode": "competitive", "round": 3}
{"type": "death_screen", "start_us": 45000000, "end_us": 47000000}
{"type": "respawn", "start_us": 47000000, "end_us": 48000000}
{"type": "gameplay", "start_us": 48000000, "end_us": 90000000, "round": 3}
```

**Approach:** Loading screen detection (low entropy frames), death screen templates, score change detection, input gap analysis (no input = likely menu/loading).

---

### Export Formats

#### Format A: HuggingFace Datasets (primary target)

```
dataset/
├── metadata.parquet          # Clip-level metadata (game, resolution, fps, duration)
├── train/
│   ├── clip_001/
│   │   ├── video.mp4         # H.264 video
│   │   ├── actions.parquet   # Per-frame action vectors (keys_held, mouse_dx, mouse_dy, etc.)
│   │   ├── captions.json     # Dense text captions
│   │   └── metadata.json     # Clip-specific metadata
│   ├── clip_002/
│   └── ...
├── dataset_card.md           # HuggingFace dataset card
└── dataset_info.json         # Schema, splits, features
```

**Why:** This is how researchers actually consume datasets. PyTorch DataLoader integration is immediate. HuggingFace is the default distribution platform for ML datasets.

#### Format B: MCAP (robotics/embodied AI)

Match the D2E/OWAMcap format for robotics researchers:

```
recording.mcap
├── /screen          # MediaRef → external .mkv
├── /keyboard        # KeyboardEvent messages
├── /mouse/raw       # RawMouseEvent messages
├── /annotations     # AnnotationEvent messages
└── /game_state      # GameStateEvent messages
```

**Why:** MCAP is the standard container for robotics data. Supporting it opens the door to the embodied AI market (robots learning from gaming data).

#### Format C: MP4 + JSON Sidecar (universal)

The simplest, most portable format:

```
clip_001/
├── video.mp4
├── actions.json      # Per-frame action dictionary (GF-Minecraft style)
├── metadata.json
├── captions.json
└── annotations.json
```

**Why:** Zero dependencies. Any researcher can load this with standard JSON/video libraries. Matches the Cosmos and Hunyuan-GameCraft conventions.

---

## Implementation Plan

### Step 9: Annotation Engine Core

**Goal:** Build the annotation pipeline infrastructure in Rust, starting with frame-indexed action conversion (Layer 1) — the highest-value, lowest-complexity annotation.

#### 9a: Frame Action Indexer

Add a new `annotation/` module to the Rust backend that converts event-stream input data into per-frame action labels.

**Files:**
- `src-tauri/src/annotation/mod.rs` — module root, `AnnotationPipeline` trait
- `src-tauri/src/annotation/frame_actions.rs` — `FrameActionIndexer`: converts `input.jsonl` event stream to per-frame action snapshots using a key/button state machine
- `src-tauri/src/annotation/types.rs` — `FrameAction`, `AnnotatedClip`, annotation data types

**Logic:**
```
Input: Vec<InputEvent> + frame_timestamps: Vec<u64> (from video frame count + fps)
Output: Vec<FrameAction>

State machine:
  - held_keys: HashSet<String>
  - held_buttons: HashSet<MouseButton>
  - mouse_pos: (f64, f64)
  - accumulated_dx, accumulated_dy: f64

For each frame timestamp:
  1. Process all input events up to this frame's timestamp
     - Key press → add to held_keys
     - Key release → remove from held_keys
     - Mouse button press → add to held_buttons, update pos
     - Mouse button release → remove from held_buttons
     - Mouse move → update pos, accumulate deltas
     - Mouse scroll → accumulate scroll delta
  2. Emit FrameAction snapshot with current state + accumulated deltas
  3. Reset accumulated deltas for next frame
```

**Tests:**
- Empty input → all-zero frame actions
- Single key press held across multiple frames
- Key press and release within one frame interval
- Mouse delta accumulation across sub-frame events
- Round-trip: frame actions → verify against known input sequence

**Checkpoint 9a:**
- [ ] `FrameActionIndexer` converts event stream to per-frame actions
- [ ] Handles all 4 InputEvent types correctly
- [ ] Mouse deltas accumulate correctly between frames
- [ ] Unit tests cover edge cases (simultaneous keys, sub-frame events)
- [ ] Serializes to JSONL matching the target format

#### 9b: Annotation File Format Extension

Extend the `.gameclip` zip format to include annotation files as optional members.

**Files:**
- `src-tauri/src/clip/format.rs` — extend `write_clip()` / `read_clip()` to handle optional annotation files
- `src-tauri/src/annotation/types.rs` — `AnnotationManifest` listing which annotation layers are present

**New optional zip members:**
```
clip.gameclip (zip)
├── metadata.json        # existing (extended with annotation_layers field)
├── input.jsonl          # existing
├── video.bin            # existing
├── audio.bin            # existing (optional)
├── thumbnail.jpg        # existing (optional)
├── frame_actions.jsonl  # NEW: per-frame action snapshots
├── captions.jsonl       # NEW: dense text captions (future)
├── game_state.jsonl     # NEW: extracted game state (future)
├── quality.json         # NEW: quality/interest scores (future)
└── episodes.jsonl       # NEW: episode segmentation (future)
```

**Checkpoint 9b:**
- [ ] Extended `.gameclip` format writes/reads annotation files
- [ ] Old clips without annotations still load correctly (backward compat)
- [ ] `metadata.json` includes `annotation_layers: ["frame_actions"]` field
- [ ] Round-trip test: write annotated clip → read back → verify annotations

#### 9c: Annotation Tauri Commands

Expose annotation operations to the frontend and CLI via Tauri IPC.

**Files:**
- `src-tauri/src/commands.rs` — add annotation commands
- `src-tauri/src/annotation/pipeline.rs` — orchestrates annotation steps

**New commands:**
```rust
#[tauri::command]
fn annotate_clip(file_path: String, layers: Vec<String>) -> Result<AnnotationResult, String>
// Run annotation pipeline on an existing clip
// layers: ["frame_actions", "quality"] — which layers to generate

#[tauri::command]
fn get_clip_annotations(file_path: String) -> Result<AnnotationData, String>
// Read all annotations from a clip

#[tauri::command]
fn get_frame_actions(file_path: String) -> Result<Vec<FrameAction>, String>
// Read per-frame action data from a clip
```

**Checkpoint 9c:**
- [ ] `annotate_clip` command runs frame action indexer on existing clips
- [ ] `get_clip_annotations` returns annotation data
- [ ] Frontend can trigger annotation and display results
- [ ] Auto-annotate option: run frame action indexer automatically on clip save

---

### Step 10: Export Pipeline

**Goal:** Export annotated clips to ML-researcher-friendly formats.

#### 10a: JSON Sidecar Export (Format C)

The simplest export format — extract clip contents into a flat directory with standard files.

**Files:**
- `src-tauri/src/export/mod.rs` — module root, `ExportFormat` enum
- `src-tauri/src/export/json_sidecar.rs` — extract to MP4 + JSON directory

**Output structure:**
```
export/clip_001/
├── video.mp4           # Extracted and re-muxed if needed
├── actions.json        # Per-frame actions as JSON array (indexed by frame number)
├── input_events.jsonl  # Raw input events (original)
├── metadata.json       # Extended metadata
└── captions.json       # Dense captions (if annotated)
```

**Checkpoint 10a:**
- [ ] Export produces valid MP4 + JSON directory
- [ ] `actions.json` uses frame-index keys matching GF-Minecraft convention
- [ ] Bulk export: process all clips in save directory
- [ ] Exported video plays in standard players

#### 10b: HuggingFace Dataset Export (Format A)

Export a collection of clips as a HuggingFace-compatible dataset.

**Files:**
- `src-tauri/src/export/huggingface.rs` — generate HuggingFace dataset structure

**Output structure:**
```
dataset/
├── metadata.csv        # One row per clip with paths and summary stats
├── data/
│   ├── clip_001/
│   │   ├── video.mp4
│   │   ├── actions.parquet   # Per-frame actions as Parquet (or .jsonl)
│   │   └── metadata.json
│   └── ...
└── README.md           # Auto-generated dataset card
```

**Checkpoint 10b:**
- [ ] Export produces valid HuggingFace dataset directory structure
- [ ] `metadata.csv` has correct schema (id, game, duration, fps, resolution, path)
- [ ] Per-clip action files are loadable by standard data libraries
- [ ] Dataset card includes schema documentation

#### 10c: Export Tauri Commands

**Files:**
- `src-tauri/src/commands.rs` — add export commands

```rust
#[tauri::command]
fn export_clips(
    clip_paths: Vec<String>,
    format: String,           // "json_sidecar" | "huggingface"
    output_dir: String,
) -> Result<ExportResult, String>
```

**Checkpoint 10c:**
- [ ] Export command works for both formats
- [ ] Progress reporting for bulk exports
- [ ] Frontend export UI (select clips, choose format, pick output directory)

---

### Step 11: Auto-Annotation on Save (Optional Enhancement)

**Goal:** Automatically run the frame action indexer when a clip is saved, so every clip ships with per-frame annotations by default.

**Files:**
- `src-tauri/src/engine.rs` — add annotation step to `save_clip()` flow
- `src-tauri/src/clip/saver.rs` — extend `ClipSaver` to include frame action generation

**Flow:**
```
Hotkey press
  → drain ring buffers
  → encode video
  → generate frame actions from input events + frame timestamps
  → write .gameclip with frame_actions.jsonl included
  → emit "clip-saved" event
```

**Checkpoint 11:**
- [ ] Every saved clip automatically includes `frame_actions.jsonl`
- [ ] No measurable impact on save latency (< 10ms for 30s clip)
- [ ] Setting to enable/disable auto-annotation

---

### Step 12: ML-Powered Annotation Layers (Future)

**Goal:** Add vision-model-powered annotations (captions, scene understanding, game state).

These layers require running ML models and are better suited as a backend service rather than running on the user's machine. They would be part of the cloud API (Step 6).

#### 12a: Caption Generation Service

- Accept uploaded clips via API
- Extract keyframes at 1-3 second intervals
- Run through vision-language model
- Return dense captions as `captions.jsonl`
- Store back into the `.gameclip` archive

#### 12b: Game State Extraction Service

- Game-specific HUD templates for top 10 games
- OCR pipeline for text extraction
- Template matching for health bars, ammo counters, minimaps
- Return structured game state as `game_state.jsonl`

#### 12c: Quality Scoring Service

- Action density analysis (input events per second)
- Visual complexity scoring (frame entropy, motion vectors)
- Highlight detection (rapid input bursts, score changes, kill feeds)
- Edge case flagging for high-value training examples

**Checkpoint 12:**
- [ ] Caption API endpoint accepts clips and returns captions
- [ ] Game state extraction works for CS2, Valorant, Fortnite (top 3)
- [ ] Quality scoring produces useful rankings across a corpus
- [ ] All annotations stored back into `.gameclip` format

---

## Priority Order

| Priority | Step | Complexity | Value |
|---|---|---|---|
| **P0** | 9a: Frame Action Indexer | Low (pure algorithm) | Very High (most-requested format) |
| **P0** | 9b: Annotation Format Extension | Low (zip members) | High (enables all future layers) |
| **P1** | 9c: Annotation Commands | Low (IPC wiring) | Medium (usability) |
| **P1** | 10a: JSON Sidecar Export | Low (file extraction) | High (universal compatibility) |
| **P1** | 11: Auto-Annotate on Save | Low (pipeline wiring) | High (every clip ships annotated) |
| **P2** | 10b: HuggingFace Export | Medium (Parquet/schema) | High (researcher distribution) |
| **P2** | 10c: Export Commands | Low (IPC wiring) | Medium (usability) |
| **P3** | 12a: Caption Generation | High (ML infra) | High (premium annotation tier) |
| **P3** | 12b: Game State Extraction | High (game-specific) | Medium (niche but valuable) |
| **P3** | 12c: Quality Scoring | Medium (heuristics + ML) | Medium (curation signal) |

**Recommended first implementation:** Steps 9a + 9b + 11 — this gives every clip automatic per-frame action labels at near-zero cost, immediately making the dataset useful for world model training. A corpus of clips with frame-indexed actions in a standard format is already more valuable than what most competitors offer.

---

## Key Insight: The Dreamer 4 Effect

Dreamer 4 showed that world models can be pretrained on massive **unlabeled** video and only need ~4% of data to have action labels. This means:

1. **Even raw gameplay video without input annotations has value** — for pretraining
2. **A small corpus of action-labeled clips can bootstrap a much larger unlabeled corpus** — via inverse dynamics models
3. **GameClip's ground-truth input labels are a moat** — they enable training inverse dynamics models that can then pseudo-label video-only gameplay from YouTube, Twitch, Medal.tv

The strategy: ship frame-indexed action labels as the core differentiator, then use those labels to train IDMs that can annotate orders of magnitude more data.
