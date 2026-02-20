import { useState, useCallback, useEffect, useRef } from "react";
import { InputOverlay } from "../components/InputOverlay";
import { useClipData } from "../hooks/useClipData";
import type { ClipSummary } from "../hooks/useClips";

interface ClipPlayerProps {
  clip: ClipSummary;
  onBack: () => void;
}

export function ClipPlayer({ clip, onBack }: ClipPlayerProps) {
  const { videoUrl, inputEvents, loading, error, loadClipData } =
    useClipData();
  const videoRef = useRef<HTMLVideoElement>(null);
  const animationRef = useRef<number | null>(null);
  const [playing, setPlaying] = useState(false);
  const [currentTimeUs, setCurrentTimeUs] = useState(0);
  const [durationUs, setDurationUs] = useState(clip.duration_secs * 1_000_000);

  useEffect(() => {
    loadClipData(clip.file_path);
  }, [clip.file_path, loadClipData]);

  const syncOverlay = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;

    const timeUs = video.currentTime * 1_000_000;
    setCurrentTimeUs(timeUs);

    if (!video.paused && !video.ended) {
      animationRef.current = requestAnimationFrame(syncOverlay);
    }
  }, []);

  const play = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    video.play().catch(() => {
      setPlaying(false);
    });
    setPlaying(true);
    animationRef.current = requestAnimationFrame(syncOverlay);
  }, [syncOverlay]);

  const pause = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    video.pause();
    setPlaying(false);
    if (animationRef.current !== null) {
      cancelAnimationFrame(animationRef.current);
      animationRef.current = null;
    }
  }, []);

  const seek = useCallback(
    (fraction: number) => {
      const video = videoRef.current;
      if (!video) return;
      const newTimeSec = fraction * (durationUs / 1_000_000);
      video.currentTime = newTimeSec;
      setCurrentTimeUs(fraction * durationUs);
    },
    [durationUs],
  );

  const handleVideoPlay = useCallback(() => {
    setPlaying(true);
    animationRef.current = requestAnimationFrame(syncOverlay);
  }, [syncOverlay]);

  const handleVideoPause = useCallback(() => {
    setPlaying(false);
    if (animationRef.current !== null) {
      cancelAnimationFrame(animationRef.current);
      animationRef.current = null;
    }
  }, []);

  const handleVideoEnded = useCallback(() => {
    setPlaying(false);
    if (animationRef.current !== null) {
      cancelAnimationFrame(animationRef.current);
      animationRef.current = null;
    }
  }, []);

  const handleLoadedMetadata = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (video.duration && isFinite(video.duration)) {
      setDurationUs(video.duration * 1_000_000);
    }
  }, []);

  useEffect(() => {
    return () => {
      if (animationRef.current !== null) {
        cancelAnimationFrame(animationRef.current);
      }
    };
  }, []);

  const progress = durationUs > 0 ? currentTimeUs / durationUs : 0;
  const playerWidth = 640;
  const playerHeight = Math.round(
    playerWidth * (clip.height / clip.width),
  );

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

      <div className="player-viewport">
        <div
          className="player-canvas"
          style={{ width: playerWidth, height: playerHeight }}
        >
          {loading && (
            <div className="player-loading">Loading clip...</div>
          )}

          {error && (
            <div className="player-error">
              Failed to load clip: {error}
            </div>
          )}

          {videoUrl && (
            <video
              ref={videoRef}
              src={videoUrl}
              width={playerWidth}
              height={playerHeight}
              onPlay={handleVideoPlay}
              onPause={handleVideoPause}
              onEnded={handleVideoEnded}
              onLoadedMetadata={handleLoadedMetadata}
              style={{ display: "block" }}
            />
          )}

          {!loading && !error && !videoUrl && (
            <div className="player-loading">No video data</div>
          )}

          <InputOverlay
            events={inputEvents}
            currentTimeUs={currentTimeUs}
            width={playerWidth}
            height={playerHeight}
            captureWidth={clip.width}
            captureHeight={clip.height}
          />
        </div>
      </div>

      <div className="player-controls">
        <button
          className="btn-play"
          onClick={playing ? pause : play}
          disabled={!videoUrl}
        >
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
