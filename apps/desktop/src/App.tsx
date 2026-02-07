import { useState } from "react";
import { useClips } from "./hooks/useClips";
import { useSettings } from "./hooks/useSettings";
import { ClipLibrary } from "./pages/ClipLibrary";
import { ClipPlayer } from "./pages/ClipPlayer";
import { SettingsPage } from "./pages/Settings";
import type { ClipSummary } from "./hooks/useClips";
import "./App.css";

type Page = "library" | "player" | "settings";

function App() {
  const [page, setPage] = useState<Page>("library");
  const [selectedClip, setSelectedClip] = useState<ClipSummary | null>(null);

  const { clips, loading: clipsLoading, error: clipsError, deleteClip, saveClip } = useClips();
  const {
    settings,
    loading: settingsLoading,
    error: settingsError,
    updateSettings,
  } = useSettings();

  const handleSelectClip = (clip: ClipSummary) => {
    setSelectedClip(clip);
    setPage("player");
  };

  const handleBack = () => {
    setPage("library");
    setSelectedClip(null);
  };

  return (
    <div className="app">
      <nav className="app-nav">
        <div className="nav-brand">GameClip</div>
        <div className="nav-links">
          <button
            className={`nav-link ${page === "library" ? "active" : ""}`}
            onClick={() => {
              setPage("library");
              setSelectedClip(null);
            }}
          >
            Library
          </button>
          <button
            className={`nav-link ${page === "settings" ? "active" : ""}`}
            onClick={() => setPage("settings")}
          >
            Settings
          </button>
        </div>
      </nav>

      <main className="app-content">
        {page === "library" && (
          <ClipLibrary
            clips={clips}
            loading={clipsLoading}
            error={clipsError}
            onSelectClip={handleSelectClip}
            onDeleteClip={deleteClip}
            onSaveClip={saveClip}
          />
        )}

        {page === "player" && selectedClip && (
          <ClipPlayer clip={selectedClip} onBack={handleBack} />
        )}

        {page === "settings" && (
          <SettingsPage
            settings={settings}
            loading={settingsLoading}
            error={settingsError}
            onUpdate={updateSettings}
            onBack={handleBack}
          />
        )}
      </main>
    </div>
  );
}

export default App;
