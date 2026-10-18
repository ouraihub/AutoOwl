import type { Diagnostics, UpdateInfo } from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

export const diagnosticsService = {
  /** App version, platform, log directory and database readiness. */
  get(): Promise<Diagnostics> {
    return client.invoke<Diagnostics>('get_diagnostics');
  },

  checkUpdate(): Promise<UpdateInfo> {
    return client.invoke<UpdateInfo>('check_update');
  }
};
