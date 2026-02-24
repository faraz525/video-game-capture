import { useState, useCallback, useEffect } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface UploadProgress {
  current_clip: number;
  total_clips: number;
  clip_name: string;
  stage: UploadStage;
  bytes_uploaded: number;
  total_bytes: number;
}

export type UploadStage =
  | "Preparing"
  | "UploadingVideo"
  | "UploadingMetadata"
  | "Committing"
  | "Done"
  | { Failed: { reason: string } };

export function useUpload() {
  const [uploading, setUploading] = useState(false);
  const [progress, setProgress] = useState<UploadProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri()) return;

    let unmounted = false;
    let unlisten: UnlistenFn | null = null;

    listen<UploadProgress>("upload-progress", (event) => {
      if (!unmounted) {
        setProgress(event.payload);
      }
    }).then((fn) => {
      if (unmounted) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      unmounted = true;
      if (unlisten) unlisten();
    };
  }, []);

  const uploadClips = useCallback(async (clipPaths: string[]) => {
    if (!isTauri()) return;
    try {
      setUploading(true);
      setError(null);
      setProgress(null);
      await invoke<number>("upload_clips", { clipPaths });
    } catch (err) {
      setError(String(err));
    } finally {
      setUploading(false);
    }
  }, []);

  const cancelUpload = useCallback(async () => {
    if (!isTauri()) return;
    try {
      await invoke("cancel_upload");
    } catch (err) {
      setError(String(err));
    }
  }, []);

  return { uploading, progress, error, uploadClips, cancelUpload };
}
