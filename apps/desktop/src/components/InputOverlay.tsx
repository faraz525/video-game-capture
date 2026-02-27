import { useMemo } from "react";

interface InputEvent {
  timestamp_us: number;
  type: string;
  key?: string;
  pressed?: boolean;
  button?: string;
  x?: number;
  y?: number;
  delta_x?: number;
  delta_y?: number;
}

interface InputOverlayProps {
  events: InputEvent[];
  currentTimeUs: number;
  width: number;
  height: number;
  captureWidth?: number;
  captureHeight?: number;
}

const VISIBLE_WINDOW_US = 500_000; // Show events within 500ms before current time
const CLICK_DISPLAY_DURATION_US = 300_000; // Click ripples stay visible for 300ms
const LOOK_AHEAD_US = 50_000; // 50ms look-ahead tolerance for timing jitter

export function InputOverlay({
  events,
  currentTimeUs,
  width,
  height,
  captureWidth = 1920,
  captureHeight = 1080,
}: InputOverlayProps) {
  const visibleEvents = useMemo(() => {
    return events.filter(
      (e) =>
        e.timestamp_us >= currentTimeUs - VISIBLE_WINDOW_US &&
        e.timestamp_us <= currentTimeUs + LOOK_AHEAD_US,
    );
  }, [events, currentTimeUs]);

  const activeKeys = useMemo(() => {
    // Walk all events up to currentTimeUs to build accurate held-key state.
    // A key is "active" if its last event before currentTimeUs was a key-down.
    const keyState = new Map<string, boolean>();
    for (const e of events) {
      if (e.timestamp_us > currentTimeUs) break;
      if (e.type === "key" && e.key) {
        keyState.set(e.key, e.pressed === true);
      }
    }
    return Array.from(keyState.entries())
      .filter(([, held]) => held)
      .map(([key]) => key);
  }, [events, currentTimeUs]);

  const mousePosition = useMemo(() => {
    const moves = visibleEvents
      .filter((e) => e.type === "mouse_move" || e.type === "mouse_button")
      .filter((e) => e.x !== undefined && e.y !== undefined);

    if (moves.length === 0) return null;
    const latest = moves[moves.length - 1];
    return {
      x: (latest.x! / captureWidth) * width,
      y: (latest.y! / captureHeight) * height,
    };
  }, [visibleEvents, width, height, captureWidth, captureHeight]);

  const mouseClicks = useMemo(() => {
    return visibleEvents
      .filter(
        (e) =>
          e.type === "mouse_button" &&
          e.pressed &&
          currentTimeUs - e.timestamp_us < CLICK_DISPLAY_DURATION_US,
      )
      .map((e) => ({
        x: ((e.x ?? 0) / captureWidth) * width,
        y: ((e.y ?? 0) / captureHeight) * height,
        button: e.button ?? "left",
      }));
  }, [visibleEvents, currentTimeUs, width, height, captureWidth, captureHeight]);

  return (
    <div className="input-overlay" style={{ width, height }}>
      {/* Key display */}
      {activeKeys.length > 0 && (
        <div className="overlay-keys">
          {activeKeys.map((key) => (
            <span key={key} className="overlay-key">
              {formatKey(key)}
            </span>
          ))}
        </div>
      )}

      {/* Mouse cursor */}
      {mousePosition && (
        <div
          className="overlay-cursor"
          style={{
            left: mousePosition.x,
            top: mousePosition.y,
          }}
        />
      )}

      {/* Mouse clicks */}
      {mouseClicks.map((click, i) => (
        <div
          key={i}
          className={`overlay-click overlay-click-${click.button}`}
          style={{
            left: click.x,
            top: click.y,
          }}
        />
      ))}
    </div>
  );
}

function formatKey(key: string): string {
  const keyMap: Record<string, string> = {
    KeyW: "W",
    KeyA: "A",
    KeyS: "S",
    KeyD: "D",
    KeyE: "E",
    Space: "SPACE",
    ShiftLeft: "SHIFT",
    ShiftRight: "SHIFT",
    ControlLeft: "CTRL",
    ControlRight: "CTRL",
    AltLeft: "ALT",
    AltRight: "ALT",
    Tab: "TAB",
    Escape: "ESC",
    Enter: "ENTER",
  };
  return keyMap[key] ?? key.replace("Key", "");
}
