import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

export function useThumbnail(filePath: string) {
  const [thumbnail, setThumbnail] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    invoke<string | null>("get_clip_thumbnail", { filePath })
      .then((result) => {
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
