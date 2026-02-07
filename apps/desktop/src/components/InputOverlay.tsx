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
}

const VISIBLE_WINDOW_US = 500_000; // Show events within 500ms of current time
const KEY_DISPLAY_DURATION_US = 300_000; // Keys stay visible for 300ms

export function InputOverlay({
  events,
  currentTimeUs,
  width,
  height,
}: InputOverlayProps) {
  const visibleEvents = useMemo(() => {
    return events.filter(
      (e) =>
        e.timestamp_us >= currentTimeUs - VISIBLE_WINDOW_US &&
        e.timestamp_us <= currentTimeUs,
    );
  }, [events, currentTimeUs]);

  const activeKeys = useMemo(() => {
    const keys = new Map<string, number>();
    for (const e of visibleEvents) {
      if (e.type === "key" && e.key && e.pressed) {
        keys.set(e.key, e.timestamp_us);
      }
    }
    // Only show keys pressed within display duration
    const result: string[] = [];
    keys.forEach((ts, key) => {
      if (currentTimeUs - ts < KEY_DISPLAY_DURATION_US) {
        result.push(key);
      }
    });
    return result;
  }, [visibleEvents, currentTimeUs]);

  const mousePosition = useMemo(() => {
    const moves = visibleEvents
      .filter((e) => e.type === "mouse_move" || e.type === "mouse_button")
      .filter((e) => e.x !== undefined && e.y !== undefined);

    if (moves.length === 0) return null;
    const latest = moves[moves.length - 1];
    return {
      x: (latest.x! / 1920) * width,
      y: (latest.y! / 1080) * height,
    };
  }, [visibleEvents, width, height]);

  const mouseClicks = useMemo(() => {
    return visibleEvents
      .filter(
        (e) =>
          e.type === "mouse_button" &&
          e.pressed &&
          currentTimeUs - e.timestamp_us < KEY_DISPLAY_DURATION_US,
      )
      .map((e) => ({
        x: ((e.x ?? 0) / 1920) * width,
        y: ((e.y ?? 0) / 1080) * height,
        button: e.button ?? "left",
      }));
  }, [visibleEvents, currentTimeUs, width, height]);

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
