import type {
  DuplicateGroupsPage,
  LocalTrack,
  Page,
  PrecheckReport,
  PrecheckSong,
  RefreshReport
} from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

export const localMusicService = {
  /** `filter` matches title/artist; omit it for the whole library. */
  list(page: number, pageSize: number, filter?: string): Promise<Page<LocalTrack>> {
    return client.invoke<Page<LocalTrack>>('get_local_music', { page, pageSize, filter });
  },

  /** Rescans the download directory and reports what changed. */
  refresh(): Promise<RefreshReport> {
    return client.invoke<RefreshReport>('refresh_local_music');
  },

  /** Removes the row; `deleteFile` also unlinks the audio file on disk. Returns the removed track. */
  remove(id: string, deleteFile?: boolean): Promise<LocalTrack> {
    return client.invoke<LocalTrack>('delete_local_music', { id, deleteFile });
  },

  get(id: string): Promise<LocalTrack> {
    return client.invoke<LocalTrack>('get_local_track', { id });
  },

  /** Tracks that look like the same recording. Groups are paged, never split. */
  duplicates(page: number, pageSize: number): Promise<DuplicateGroupsPage> {
    return client.invoke<DuplicateGroupsPage>('get_local_duplicates', { page, pageSize });
  },

  /** How many of `songs` the library already has, before a batch download runs. */
  precheck(songs: PrecheckSong[]): Promise<PrecheckReport> {
    return client.invoke<PrecheckReport>('precheck_downloads', { songs });
  }
};
