import { useThumbnail } from "../hooks/useThumbnail";
import type { ClipSummary } from "../hooks/useClips";

interface ClipCardProps {
  clip: ClipSummary;
  onSelect: (clip: ClipSummary) => void;
  onDelete: (filePath: string) => void;
}

function formatDuration(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function ClipCard({ clip, onSelect, onDelete }: ClipCardProps) {
  const thumbnail = useThumbnail(clip.file_path);

  return (
    <div className="clip-card" onClick={() => onSelect(clip)}>
      <div className="clip-thumbnail">
        {thumbnail ? (
          <>
            <img
              className="clip-thumbnail-img"
              src={thumbnail}
              alt={clip.name}
            />
            <span className="clip-duration">
              {formatDuration(clip.duration_secs)}
            </span>
          </>
        ) : (
          <div className="clip-thumbnail-placeholder">
            <span className="clip-duration">
              {formatDuration(clip.duration_secs)}
            </span>
          </div>
        )}
      </div>
      <div className="clip-info">
        <div className="clip-name">{clip.name}</div>
        <div className="clip-meta">
          {clip.game && <span className="clip-game">{clip.game}</span>}
          <span className="clip-resolution">
            {clip.width}x{clip.height}
          </span>
          <span className="clip-fps">{clip.fps}fps</span>
          {clip.has_audio && <span className="clip-audio">audio</span>}
        </div>
        <div className="clip-meta">
          <span>{clip.input_event_count} inputs</span>
          <span>{formatDate(clip.created_at)}</span>
        </div>
      </div>
      <button
        className="clip-delete"
        onClick={(e) => {
          e.stopPropagation();
          onDelete(clip.file_path);
        }}
        title="Delete clip"
      >
        x
      </button>
    </div>
  );
}
