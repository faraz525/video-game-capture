#!/bin/bash
# GameClip Demo — Print annotated clip data in a presentation-ready format
# Usage: ./scripts/demo-print.sh [path-to-clip]

set -euo pipefail

CLIP="${1:-$(ls -t ~/GameClip/clips/*.gameclip 2>/dev/null | head -1)}"

if [ -z "$CLIP" ] || [ ! -f "$CLIP" ]; then
  echo "No clip found."
  exit 1
fi

BOLD="\033[1m"
DIM="\033[2m"
CYAN="\033[36m"
GREEN="\033[32m"
YELLOW="\033[33m"
MAGENTA="\033[35m"
WHITE="\033[97m"
RESET="\033[0m"

META=$(unzip -p "$CLIP" metadata.json)
QUALITY=$(unzip -p "$CLIP" quality.json 2>/dev/null || echo "{}")
FRAME_COUNT=$(unzip -p "$CLIP" frame_actions.jsonl 2>/dev/null | wc -l | tr -d ' ')
EVENT_COUNT=$(unzip -p "$CLIP" input.jsonl | wc -l | tr -d ' ')
FILE_SIZE=$(du -h "$CLIP" | cut -f1 | tr -d ' ')

# Parse metadata fields
CLIP_NAME=$(echo "$META" | python3 -c "import sys,json; print(json.load(sys.stdin)['name'])")
GAME=$(echo "$META" | python3 -c "import sys,json; print(json.load(sys.stdin).get('game') or 'Not detected')")
WIDTH=$(echo "$META" | python3 -c "import sys,json; print(json.load(sys.stdin)['width'])")
HEIGHT=$(echo "$META" | python3 -c "import sys,json; print(json.load(sys.stdin)['height'])")
FPS=$(echo "$META" | python3 -c "import sys,json; print(json.load(sys.stdin)['fps'])")
DURATION=$(echo "$META" | python3 -c "import sys,json; d=json.load(sys.stdin)['duration_secs']; print(f'{d:.1f}s')")
HAS_AUDIO=$(echo "$META" | python3 -c "import sys,json; print('Yes' if json.load(sys.stdin)['has_audio'] else 'No')")
KEYBOARD=$(echo "$META" | python3 -c "import sys,json; print('Yes' if json.load(sys.stdin)['devices']['keyboard'] else 'No')")
MOUSE=$(echo "$META" | python3 -c "import sys,json; print('Yes' if json.load(sys.stdin)['devices']['mouse'] else 'No')")
LAYERS=$(echo "$META" | python3 -c "import sys,json; print(', '.join(json.load(sys.stdin).get('annotation_layers',[]) or ['none']))")

# Parse quality fields
OVERALL=$(echo "$QUALITY" | python3 -c "import sys,json; q=json.load(sys.stdin); print(f\"{q.get('overall_score',0):.2f}\")" 2>/dev/null || echo "N/A")
GENRE=$(echo "$QUALITY" | python3 -c "import sys,json; print(json.load(sys.stdin).get('genre','N/A'))" 2>/dev/null || echo "N/A")

echo ""
echo -e "${BOLD}${CYAN}================================================================${RESET}"
echo -e "${BOLD}${CYAN}  GAMECLIP — Annotated Training Data                            ${RESET}"
echo -e "${BOLD}${CYAN}================================================================${RESET}"
echo ""
echo -e "${BOLD}${WHITE}  CLIP METADATA${RESET}"
echo -e "${DIM}  ────────────────────────────────────────────────────────────${RESET}"
echo -e "  Name:             ${BOLD}${CLIP_NAME}${RESET}"
echo -e "  Game:             ${GAME}"
echo -e "  Resolution:       ${WIDTH}x${HEIGHT} @ ${FPS}fps"
echo -e "  Duration:         ${DURATION}"
echo -e "  File size:        ${FILE_SIZE}"
echo -e "  Audio:            ${HAS_AUDIO} (48kHz stereo)"
echo -e "  Keyboard:         ${KEYBOARD}"
echo -e "  Mouse:            ${MOUSE}"
echo -e "  Annotation layers:${GREEN} ${LAYERS}${RESET}"
echo ""
echo -e "${BOLD}${WHITE}  DATA VOLUME${RESET}"
echo -e "${DIM}  ────────────────────────────────────────────────────────────${RESET}"
echo -e "  Raw input events: ${BOLD}${EVENT_COUNT}${RESET}"
echo -e "  Frame actions:    ${BOLD}${FRAME_COUNT}${RESET} (one per video frame)"
echo ""
echo -e "${BOLD}${WHITE}  QUALITY SCORE${RESET}"
echo -e "${DIM}  ────────────────────────────────────────────────────────────${RESET}"
echo -e "  Overall:          ${BOLD}${YELLOW}${OVERALL}${RESET}"
echo -e "  Genre:            ${GENRE}"

echo "$QUALITY" | python3 -c "
import sys, json
q = json.load(sys.stdin)
ds = q.get('dimension_scores', {})
if ds:
    for k, v in ds.items():
        label = k.replace('_', ' ').title()
        bar_len = int(v * 20)
        bar = '\033[32m' + '█' * bar_len + '\033[2m' + '░' * (20 - bar_len) + '\033[0m'
        print(f'  {label:<22s}{bar}  {v:.2f}')
" 2>/dev/null

# Highlights
HIGHLIGHT_COUNT=$(echo "$QUALITY" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('highlights',[])))" 2>/dev/null || echo "0")
EDGE_CASES=$(echo "$QUALITY" | python3 -c "import sys,json; ec=json.load(sys.stdin).get('edge_case_flags',[]); print(', '.join(ec) if ec else 'none')" 2>/dev/null || echo "none")
echo ""
echo -e "  Highlights found: ${BOLD}${HIGHLIGHT_COUNT}${RESET}"
echo -e "  Edge cases:       ${EDGE_CASES}"

echo ""
echo -e "${BOLD}${WHITE}  SAMPLE FRAME ACTIONS${RESET}"
echo -e "${DIM}  ────────────────────────────────────────────────────────────${RESET}"

unzip -p "$CLIP" frame_actions.jsonl 2>/dev/null | python3 -c "
import sys, json

frames = []
for line in sys.stdin:
    frames.append(json.loads(line.strip()))

# Pick 3 interesting frames: one with keys, one with mouse button, one with movement
examples = []
for f in frames:
    if f.get('keys_held') and len(examples) < 1:
        examples.append(f)
    elif f.get('mouse_buttons_held') and len(examples) < 2:
        examples.append(f)
    elif (abs(f.get('mouse_dx',0)) > 5 or abs(f.get('mouse_dy',0)) > 5) and len(examples) < 3:
        examples.append(f)

# Fallback: just take evenly spaced frames
if len(examples) < 3:
    step = max(1, len(frames) // 4)
    for i in range(0, len(frames), step):
        if len(examples) >= 3:
            break
        if frames[i] not in examples:
            examples.append(frames[i])

for i, f in enumerate(examples[:3]):
    keys = ', '.join(f.get('keys_held',[])) or '—'
    btns = ', '.join(f.get('mouse_buttons_held',[])) or '—'
    dx = f.get('mouse_dx', 0)
    dy = f.get('mouse_dy', 0)
    ts_sec = f.get('timestamp_us', 0) / 1_000_000

    print(f'  \033[1m\033[35mFrame {f[\"frame\"]:>4d}\033[0m  t={ts_sec:.2f}s')
    print(f'    Keys held:      {keys}')
    print(f'    Mouse buttons:  {btns}')
    print(f'    Mouse delta:    dx={dx:+.1f}  dy={dy:+.1f}')
    print(f'    Cursor pos:     ({f.get(\"mouse_x\",0):.0f}, {f.get(\"mouse_y\",0):.0f})')
    print()
"

echo -e "${BOLD}${WHITE}  RAW INPUT EVENTS (first 5)${RESET}"
echo -e "${DIM}  ────────────────────────────────────────────────────────────${RESET}"

unzip -p "$CLIP" input.jsonl | head -5 | python3 -c "
import sys, json
for line in sys.stdin:
    e = json.loads(line.strip())
    t = e['timestamp_us'] / 1_000_000
    etype = e['type']
    if etype == 'key':
        action = 'DOWN' if e.get('pressed') else 'UP'
        print(f'  \033[2m{t:>7.3f}s\033[0m  {etype:<14s} {e[\"key\"]} {action}')
    elif etype == 'mouse_move':
        print(f'  \033[2m{t:>7.3f}s\033[0m  {etype:<14s} ({e[\"x\"]:.0f}, {e[\"y\"]:.0f})')
    elif etype == 'mouse_button':
        action = 'DOWN' if e.get('pressed') else 'UP'
        print(f'  \033[2m{t:>7.3f}s\033[0m  {etype:<14s} {e.get(\"button\",\"?\")} {action} at ({e.get(\"x\",0):.0f}, {e.get(\"y\",0):.0f})')
    elif etype == 'mouse_scroll':
        print(f'  \033[2m{t:>7.3f}s\033[0m  {etype:<14s} dx={e.get(\"delta_x\",0):.1f} dy={e.get(\"delta_y\",0):.1f}')
    else:
        print(f'  \033[2m{t:>7.3f}s\033[0m  {etype}')
"

echo ""
echo -e "${BOLD}${WHITE}  .GAMECLIP ARCHIVE CONTENTS${RESET}"
echo -e "${DIM}  ────────────────────────────────────────────────────────────${RESET}"
unzip -l "$CLIP" | python3 -c "
import sys
lines = sys.stdin.read().strip().split('\n')
for line in lines:
    line = line.strip()
    parts = line.split()
    if len(parts) >= 4 and parts[0].isdigit() and '.' in parts[3]:
        name = parts[3]
        size_kb = int(parts[0]) / 1024
        print(f'  {name:<28s} {size_kb:>8.1f} KB')
"

echo ""
echo -e "${BOLD}${CYAN}================================================================${RESET}"
echo -e "${DIM}  One hotkey press. Zero annotation effort.${RESET}"
echo -e "${DIM}  Every clip is ML-ready training data.${RESET}"
echo -e "${BOLD}${CYAN}================================================================${RESET}"
echo ""
