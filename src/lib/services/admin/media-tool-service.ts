import type { MediaToolStatus } from '$lib/domain/contracts';
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
  }
};
