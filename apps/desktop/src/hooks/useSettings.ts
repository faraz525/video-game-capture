import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface AppSettings {
  buffer_duration_secs: number;
  save_directory: string;
  hotkey: string;
  capture_fps: number;
  capture_width: number;
  capture_height: number;
}

export function useSettings() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchSettings = useCallback(async () => {
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
