import type { MediaToolStatus } from '$lib/domain/contracts';
import type { Unlisten } from '$lib/services/tauri-client';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

export const mediaToolService = {
  /**
   * Rust returns `MediaToolStatus` directly rather than a `Result`, so a missing
   * ffmpeg/ffprobe arrives as `false` plus an error string on the status itself
   * instead of a rejection.
   */
  getStatus(): Promise<MediaToolStatus> {
    return client.invoke<MediaToolStatus>('get_media_tool_status');
  },

  /** Runs `scoop install ffmpeg`; rejects with the reason when Scoop is missing or fails. */
  installWithScoop(): Promise<MediaToolStatus> {
    return client.invoke<MediaToolStatus>('install_media_tools');
  },

  /** Downloads the pinned portable ffmpeg into the data dir (GitHub proxy if enabled). */
  downloadPortable(): Promise<MediaToolStatus> {
    return client.invoke<MediaToolStatus>('install_portable_ffmpeg');
  },

  onDownloadProgress(handler: (received: number, total: number) => void): Promise<Unlisten> {
    return client.listen<{ received: number; total: number }>('media:ffmpeg-progress', (event) =>
      handler(event.payload.received, event.payload.total)
    );
  }
};
