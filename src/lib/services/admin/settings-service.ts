import type { AppSettings, StoragePaths } from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

export const settingsService = {
  get(): Promise<AppSettings> {
    return client.invoke<AppSettings>('get_settings');
  },

  /** Takes the whole settings object; returns the persisted (validated) result. */
  update(settings: AppSettings): Promise<AppSettings> {
    return client.invoke<AppSettings>('update_settings', { settings });
  },

  storagePaths(): Promise<StoragePaths> {
    return client.invoke<StoragePaths>('get_storage_paths');
  },

  /** Empty string restores the default; the backend rejects unwritable folders. */
  setStoragePaths(dataDir: string, logsDir: string): Promise<StoragePaths> {
    return client.invoke<StoragePaths>('set_storage_paths', { dataDir, logsDir });
  },

  /** Native folder picker; `null` when cancelled. */
  pickFolder(start?: string): Promise<string | null> {
    return client.invoke<string | null>('pick_folder', { start: start || null });
  },

  restart(): Promise<void> {
    return client.invoke<void>('restart_app');
  }
};
