import { useState, useCallback, useEffect, useRef } from "react";
import { InputOverlay } from "../components/InputOverlay";
import type { ClipSummary } from "../hooks/useClips";

interface InputEvent {
  timestamp_us: number;
  type: string;
  key?: string;
  pressed?: boolean;
  button?: string;
  x?: number;
  y?: number;
}

interface ClipPlayerProps {
  clip: ClipSummary;
  onBack: () => void;
}

export function ClipPlayer({ clip, onBack }: ClipPlayerProps) {
  const [playing, setPlaying] = useState(false);
  const [currentTimeUs, setCurrentTimeUs] = useState(0);
  const [inputEvents] = useState<InputEvent[]>(() => generateMockInputEvents(clip));
  const animationRef = useRef<number | null>(null);
  const startTimeRef = useRef<number | null>(null);
  const playerRef = useRef<HTMLDivElement>(null);

  const durationUs = clip.duration_secs * 1_000_000;

  const play = useCallback(() => {
    if (currentTimeUs >= durationUs) {
      setCurrentTimeUs(0);
    }
    setPlaying(true);
    startTimeRef.current = performance.now() - (currentTimeUs / 1000);
  }, [currentTimeUs, durationUs]);

  const pause = useCallback(() => {
    setPlaying(false);
    if (animationRef.current !== null) {
      cancelAnimationFrame(animationRef.current);
      animationRef.current = null;
    }
  }, []);

  const seek = useCallback(
    (fraction: number) => {
      const newTime = fraction * durationUs;
      setCurrentTimeUs(newTime);
      if (playing) {
        startTimeRef.current = performance.now() - (newTime / 1000);
      }
    },
    [durationUs, playing],
  );

  useEffect(() => {
    if (!playing) return;

    const tick = () => {
      if (!startTimeRef.current) return;
      const elapsed = (performance.now() - startTimeRef.current) * 1000; // ms to us
      if (elapsed >= durationUs) {
        setCurrentTimeUs(durationUs);
        setPlaying(false);
        return;
      }
      setCurrentTimeUs(elapsed);
      animationRef.current = requestAnimationFrame(tick);
    };

    animationRef.current = requestAnimationFrame(tick);

    return () => {
      if (animationRef.current !== null) {
        cancelAnimationFrame(animationRef.current);
      }
    };
  }, [playing, durationUs]);

  const progress = durationUs > 0 ? currentTimeUs / durationUs : 0;
  const playerWidth = 640;
  const playerHeight = 480;

  return (
    <div className="clip-player">
      <div className="player-header">
        <button className="btn-back" onClick={onBack}>
          Back
        </button>
        <h2>{clip.name}</h2>
        <div className="player-meta">
          {clip.game && <span className="badge">{clip.game}</span>}
          <span className="badge">
            {clip.width}x{clip.height}
          </span>
          <span className="badge">{clip.fps}fps</span>
          <span className="badge">{clip.input_event_count} inputs</span>
        </div>
      </div>

      <div className="player-viewport" ref={playerRef}>
        <div
          className="player-canvas"
          style={{ width: playerWidth, height: playerHeight }}
        >
          {/* Mock video: cycling background color */}
          <div
            className="mock-video"
            style={{
              width: playerWidth,
              height: playerHeight,
              backgroundColor: getMockColor(currentTimeUs),
            }}
          >
            <span className="mock-label">Mock Capture</span>
          </div>

          {/* Input overlay */}
          <InputOverlay
            events={inputEvents}
            currentTimeUs={currentTimeUs}
            width={playerWidth}
            height={playerHeight}
          />
        </div>
      </div>

      {/* Playback controls */}
      <div className="player-controls">
        <button className="btn-play" onClick={playing ? pause : play}>
          {playing ? "Pause" : "Play"}
        </button>

        <div
          className="progress-bar"
          onClick={(e) => {
            const rect = e.currentTarget.getBoundingClientRect();
            const fraction = (e.clientX - rect.left) / rect.width;
            seek(Math.max(0, Math.min(1, fraction)));
          }}
        >
          <div
            className="progress-fill"
            style={{ width: `${progress * 100}%` }}
          />
        </div>

        <span className="time-display">
          {formatTime(currentTimeUs)} / {formatTime(durationUs)}
        </span>
      </div>
    </div>
  );
}

function formatTime(us: number): string {
  const totalSecs = Math.floor(us / 1_000_000);
  const mins = Math.floor(totalSecs / 60);
  const secs = totalSecs % 60;
  return `${mins}:${secs.toString().padStart(2, "0")}`;
}

function getMockColor(timeUs: number): string {
  const colors = ["#e74c3c", "#2ecc71", "#3498db", "#f1c40f"];
  const idx = Math.floor(timeUs / 500_000) % colors.length;
  return colors[idx];
}

function generateMockInputEvents(clip: ClipSummary): InputEvent[] {
  const events: InputEvent[] = [];
  const durationUs = clip.duration_secs * 1_000_000;
  const keys = ["KeyW", "KeyA", "KeyS", "KeyD", "Space"];

  for (let t = 0; t < durationUs; t += 100_000) {
    // Key press every 100ms
    const key = keys[Math.floor(t / 100_000) % keys.length];
    events.push({
      timestamp_us: t,
      type: "key",
      key,
      pressed: true,
    });
    events.push({
      timestamp_us: t + 80_000,
      type: "key",
      key,
      pressed: false,
    });

    // Mouse move every 200ms
    if (t % 200_000 === 0) {
      events.push({
        timestamp_us: t,
        type: "mouse_move",
        x: 960 + Math.sin(t / 1_000_000) * 400,
        y: 540 + Math.cos(t / 1_000_000) * 300,
      });
    }

    // Mouse click every 500ms
    if (t % 500_000 === 0) {
      events.push({
        timestamp_us: t,
        type: "mouse_button",
        button: "left",
        pressed: true,
        x: 960 + Math.sin(t / 1_000_000) * 400,
        y: 540 + Math.cos(t / 1_000_000) * 300,
      });
    }
  }

  return events;
}
