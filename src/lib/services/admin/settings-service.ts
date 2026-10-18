import type { AppSettings } from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

export const settingsService = {
  get(): Promise<AppSettings> {
    return client.invoke<AppSettings>('get_settings');
  },

  /** Takes the whole settings object; returns the persisted (validated) result. */
  update(settings: AppSettings): Promise<AppSettings> {
    return client.invoke<AppSettings>('update_settings', { settings });
  }
};
