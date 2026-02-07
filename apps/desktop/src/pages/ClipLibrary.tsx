import { ClipCard } from "../components/ClipCard";
import type { ClipSummary } from "../hooks/useClips";

interface ClipLibraryProps {
  clips: ClipSummary[];
  loading: boolean;
  error: string | null;
  onSelectClip: (clip: ClipSummary) => void;
  onDeleteClip: (filePath: string) => void;
  onSaveClip: () => void;
}

export function ClipLibrary({
  clips,
  loading,
  error,
  onSelectClip,
  onDeleteClip,
  onSaveClip,
}: ClipLibraryProps) {
  return (
    <div className="clip-library">
      <div className="library-header">
        <h2>Clip Library</h2>
        <button className="btn-primary" onClick={onSaveClip}>
          Save Clip
        </button>
      </div>

      {error && <div className="error-banner">{error}</div>}

      {loading ? (
        <div className="loading">Loading clips...</div>
      ) : clips.length === 0 ? (
        <div className="empty-state">
          <div className="empty-icon">&#127916;</div>
          <h3>No clips yet</h3>
          <p>
            Press <kbd>Ctrl+Shift+R</kbd> to save a clip, or click the button
            above.
          </p>
        </div>
      ) : (
        <div className="clip-grid">
          {clips.map((clip) => (
            <ClipCard
              key={clip.id}
              clip={clip}
              onSelect={onSelectClip}
              onDelete={onDeleteClip}
            />
          ))}
        </div>
      )}
    </div>
  );
}
