import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface ClipSummary {
  id: string;
  name: string;
  game: string | null;
  duration_secs: number;
  created_at: string;
  file_path: string;
  input_event_count: number;
  has_audio: boolean;
  width: number;
  height: number;
  fps: number;
}

export function useClips() {
  const [clips, setClips] = useState<ClipSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchClips = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);
      const result = await invoke<ClipSummary[]>("list_clips");
      setClips(result);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const deleteClip = useCallback(
    async (filePath: string) => {
      try {
        await invoke("delete_clip", { filePath });
        await fetchClips();
      } catch (err) {
        setError(String(err));
      }
    },
    [fetchClips],
  );

  const saveClip = useCallback(async () => {
    try {
      const path = await invoke<string>("save_clip");
      await fetchClips();
      return path;
    } catch (err) {
      setError(String(err));
      return null;
    }
  }, [fetchClips]);

  useEffect(() => {
    fetchClips();
  }, [fetchClips]);

  useEffect(() => {
    const unlisten = listen("clip-saved", () => {
      fetchClips();
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchClips]);

  return { clips, loading, error, fetchClips, deleteClip, saveClip };
}
