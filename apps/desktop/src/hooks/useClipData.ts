import { useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { convertFileSrc } from "@tauri-apps/api/core";

export interface ClipInputEvent {
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

interface ClipData {
  videoUrl: string | null;
  inputEvents: ClipInputEvent[];
  loading: boolean;
  error: string | null;
}

function normalizeTimestamps(events: ClipInputEvent[]): ClipInputEvent[] {
  if (events.length === 0) return events;

  const minTs = events.reduce(
    (min, e) => Math.min(min, e.timestamp_us),
    events[0].timestamp_us,
  );

  return events.map((e) => ({
    ...e,
    timestamp_us: e.timestamp_us - minTs,
  }));
}

export function useClipData() {
  const [data, setData] = useState<ClipData>({
    videoUrl: null,
    inputEvents: [],
    loading: false,
    error: null,
  });

  const loadClipData = useCallback(async (filePath: string) => {
    setData({ videoUrl: null, inputEvents: [], loading: true, error: null });

    try {
      const [tempPath, rawEvents] = await Promise.all([
        invoke<string>("extract_clip_video", { filePath }),
        invoke<ClipInputEvent[]>("get_clip_input_events", { filePath }),
      ]);

      const videoUrl = convertFileSrc(tempPath);
      const inputEvents = normalizeTimestamps(rawEvents);

      setData({ videoUrl, inputEvents, loading: false, error: null });
    } catch (err) {
      setData({
        videoUrl: null,
        inputEvents: [],
        loading: false,
        error: String(err),
      });
    }
  }, []);

  return { ...data, loadClipData };
}
