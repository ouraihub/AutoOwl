import type { ModelRef } from '@ouraihub/aether-ui/ports';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

export const chatService = {
  /** Model the chat starts with: the last one used, else the first usable one; `null` if none. */
  defaultModel(): Promise<ModelRef | null> {
    return client.invoke<ModelRef | null>('chat_default_model');
  }
};
