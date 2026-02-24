import { useState, useEffect, useCallback } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

export interface HuggingFaceConfig {
  upload_consent: boolean;
  /** Token is write-only — the backend skips serializing it for security.
   *  When reading settings, this will be empty. Only set when saving. */
  token: string;
  repo_id: string;
  quality_gate: number;
  private_repo: boolean;
}

export interface AppSettings {
  buffer_duration_secs: number;
  save_directory: string;
  hotkey: string;
  capture_fps: number;
  capture_width: number;
  capture_height: number;
  huggingface: HuggingFaceConfig;
}

export function useSettings() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchSettings = useCallback(async () => {
    if (!isTauri()) {
      setLoading(false);
      setError("Not running inside Tauri — settings IPC unavailable");
      return;
    }
    try {
      setLoading(true);
      setError(null);
      const result = await invoke<AppSettings>("get_settings");
      setSettings(result);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const updateSettings = useCallback(
    async (newSettings: AppSettings) => {
      if (!isTauri()) return;
      try {
        setError(null);
        await invoke("update_settings", { newSettings });
        setSettings(newSettings);
      } catch (err) {
        setError(String(err));
      }
    },
    [],
  );

  useEffect(() => {
    fetchSettings();
  }, [fetchSettings]);

  return { settings, loading, error, updateSettings };
}
