import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useUpload, type UploadProgress } from "./useUpload";

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

describe("useUpload", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListen.mockResolvedValue(vi.fn());
  });

  // T42: uploadClips sets uploading=true
  it("sets uploading to true when uploadClips is called", async () => {
    let resolveInvoke: (value: unknown) => void = () => {};
    mockInvoke.mockImplementation(
      () => new Promise((resolve) => { resolveInvoke = resolve; }),
    );

    const { result } = renderHook(() => useUpload());

    expect(result.current.uploading).toBe(false);

    // Start the upload but don't await — invoke stays pending
    let uploadPromise: Promise<void> | undefined;
    act(() => {
      uploadPromise = result.current.uploadClips(["/path/to/clip.gameclip"]);
    });

    // After the synchronous setUploading(true) flushes, uploading should be true
    expect(result.current.uploading).toBe(true);
    expect(mockInvoke).toHaveBeenCalledWith("upload_clips", {
      clipPaths: ["/path/to/clip.gameclip"],
    });

    // Resolve the invoke and let the finally block run
    await act(async () => {
      resolveInvoke(1);
      await uploadPromise;
    });

    expect(result.current.uploading).toBe(false);
  });

  // T43: error state set on invoke rejection
  it("sets error when invoke rejects", async () => {
    mockInvoke.mockRejectedValue(new Error("Upload failed: network error"));

    const { result } = renderHook(() => useUpload());

    await act(async () => {
      await result.current.uploadClips(["/path/to/clip.gameclip"]);
    });

    expect(result.current.error).toBe("Error: Upload failed: network error");
    expect(result.current.uploading).toBe(false);
  });

  // T44: progress state updates on upload-progress event
  it("updates progress when upload-progress event fires", async () => {
    type ListenerCallback = (event: { payload: UploadProgress }) => void;
    let capturedListener: ListenerCallback | null = null;

    mockListen.mockImplementation((_event: string, handler: unknown) => {
      capturedListener = handler as ListenerCallback;
      return Promise.resolve(vi.fn());
    });

    const { result } = renderHook(() => useUpload());

    // Wait for the listen setup
    await act(async () => {
      await Promise.resolve();
    });

    expect(capturedListener).not.toBeNull();

    const progressPayload: UploadProgress = {
      current_clip: 0,
      total_clips: 2,
      clip_name: "test_clip",
      stage: "UploadingVideo",
      bytes_uploaded: 1024,
      total_bytes: 4096,
    };

    act(() => {
      capturedListener!({ payload: progressPayload });
    });

    expect(result.current.progress).toEqual(progressPayload);
  });
});
