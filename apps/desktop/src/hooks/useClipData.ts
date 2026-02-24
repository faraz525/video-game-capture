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

function normalizeTimestamps(
  events: ClipInputEvent[],
  videoStartTimestampUs: number,
): ClipInputEvent[] {
  if (events.length === 0) return events;

  // Use the first video frame timestamp as the origin so input events
  // align with video.currentTime (which starts at 0 = first frame).
  // For old clips where videoStartTimestampUs is 0, fall back to the
  // minimum input event timestamp (slightly imprecise but close enough).
  const origin =
    videoStartTimestampUs > 0
      ? videoStartTimestampUs
      : events.reduce(
          (min, e) => Math.min(min, e.timestamp_us),
          events[0].timestamp_us,
        );

  return events.map((e) => ({
    ...e,
    timestamp_us: e.timestamp_us - origin,
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
      const [tempPath, clipInputData] = await Promise.all([
        invoke<string>("extract_clip_video", { filePath }),
        invoke<{ events: ClipInputEvent[]; video_start_timestamp_us: number }>(
          "get_clip_input_events",
          { filePath },
        ),
      ]);

      const videoUrl = convertFileSrc(tempPath);
      const inputEvents = normalizeTimestamps(
        clipInputData.events,
        clipInputData.video_start_timestamp_us,
      );

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
