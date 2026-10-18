import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

export const healthService = {
  /** Backend liveness probe; resolves to a short status string. */
  check(): Promise<string> {
    return client.invoke<string>('health');
  }
};
