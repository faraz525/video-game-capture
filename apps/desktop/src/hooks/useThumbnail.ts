import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

// Module-level cache: persists across component mounts/unmounts so
// thumbnails loaded once don't need another IPC round-trip when
// ClipCards re-render or remount (e.g., after a clip list refresh).
const thumbnailCache = new Map<string, string | null>();

export function useThumbnail(filePath: string) {
  const [thumbnail, setThumbnail] = useState<string | null>(
    () => thumbnailCache.get(filePath) ?? null,
  );

  useEffect(() => {
    // Return cached value immediately without IPC
    if (thumbnailCache.has(filePath)) {
      setThumbnail(thumbnailCache.get(filePath) ?? null);
      return;
    }

    let cancelled = false;

    invoke<string | null>("get_clip_thumbnail", { filePath })
      .then((result) => {
        thumbnailCache.set(filePath, result);
        if (!cancelled) {
          setThumbnail(result);
        }
      })
      .catch(() => {
        // Silently ignore thumbnail load failures
      });

    return () => {
      cancelled = true;
    };
  }, [filePath]);

  return thumbnail;
}
