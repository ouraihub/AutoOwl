import type { Lyrics, SongRef } from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

export const lyricsService = {
  /** Fetches lyrics by track identity, not by full `Song`. */
  get(song: SongRef): Promise<Lyrics> {
    return client.invoke<Lyrics>('get_lyrics', { song });
  },

  /** Re-parses and re-sorts lines, e.g. after an LRC paste. */
  normalize(request: Lyrics): Promise<Lyrics> {
    return client.invoke<Lyrics>('normalize_lyrics_command', { request });
  }
};
