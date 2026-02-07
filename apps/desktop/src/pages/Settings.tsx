import { useState, useEffect } from "react";
import type { AppSettings } from "../hooks/useSettings";

interface SettingsPageProps {
  settings: AppSettings | null;
  loading: boolean;
  error: string | null;
  onUpdate: (settings: AppSettings) => void;
  onBack: () => void;
}

export function SettingsPage({
  settings,
  loading,
  error,
  onUpdate,
  onBack,
}: SettingsPageProps) {
  const [draft, setDraft] = useState<AppSettings | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (settings) {
      setDraft({ ...settings });
    }
  }, [settings]);

  if (loading || !draft) {
    return <div className="loading">Loading settings...</div>;
  }

  const handleSave = () => {
    onUpdate(draft);
    setSaved(true);
    setTimeout(() => setSaved(false), 2000);
  };

  const updateField = <K extends keyof AppSettings>(
    key: K,
    value: AppSettings[K],
  ) => {
    setDraft({ ...draft, [key]: value });
  };

  return (
    <div className="settings-page">
      <div className="settings-header">
        <button className="btn-back" onClick={onBack}>
          Back
        </button>
        <h2>Settings</h2>
      </div>

      {error && <div className="error-banner">{error}</div>}

      <div className="settings-form">
        <div className="setting-group">
          <h3>Capture</h3>

          <label className="setting-label">
            <span>Hotkey</span>
            <input
              type="text"
              value={draft.hotkey}
              onChange={(e) => updateField("hotkey", e.target.value)}
              className="setting-input"
            />
          </label>

          <label className="setting-label">
            <span>Buffer Duration (seconds)</span>
            <input
              type="number"
              min={5}
              max={120}
              value={draft.buffer_duration_secs}
              onChange={(e) =>
                updateField("buffer_duration_secs", parseInt(e.target.value) || 30)
              }
              className="setting-input"
            />
          </label>

          <label className="setting-label">
            <span>Target FPS</span>
            <select
              value={draft.capture_fps}
              onChange={(e) =>
                updateField("capture_fps", parseInt(e.target.value))
              }
              className="setting-input"
            >
              <option value={30}>30 fps</option>
              <option value={60}>60 fps</option>
              <option value={120}>120 fps</option>
            </select>
          </label>

          <label className="setting-label">
            <span>Resolution</span>
            <select
              value={`${draft.capture_width}x${draft.capture_height}`}
              onChange={(e) => {
                const [w, h] = e.target.value.split("x").map(Number);
                updateField("capture_width", w);
                updateField("capture_height", h);
              }}
              className="setting-input"
            >
              <option value="1920x1080">1920x1080 (1080p)</option>
              <option value="2560x1440">2560x1440 (1440p)</option>
              <option value="3840x2160">3840x2160 (4K)</option>
            </select>
          </label>
        </div>

        <div className="setting-group">
          <h3>Storage</h3>

          <label className="setting-label">
            <span>Save Directory</span>
            <input
              type="text"
              value={draft.save_directory}
              onChange={(e) => updateField("save_directory", e.target.value)}
              className="setting-input setting-input-wide"
            />
          </label>
        </div>

        <div className="settings-actions">
          <button className="btn-primary" onClick={handleSave}>
            {saved ? "Saved!" : "Save Settings"}
          </button>
        </div>
      </div>
    </div>
  );
}
