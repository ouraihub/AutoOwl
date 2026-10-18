import type { Page, PlaybackEntry, Song } from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

export const historyService = {
  /** Appends one play. Returns the stored entry, newest first in listings. */
  record(song: Song): Promise<PlaybackEntry> {
    return client.invoke<PlaybackEntry>('record_playback', { song });
  },

  list(page: number, pageSize: number): Promise<Page<PlaybackEntry>> {
    return client.invoke<Page<PlaybackEntry>>('list_playback_history', { page, pageSize });
  },

  /** Returns how many entries were deleted. */
  clear(): Promise<number> {
    return client.invoke<number>('clear_playback_history');
  }
};
