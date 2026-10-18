import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen as tauriListen } from '@tauri-apps/api/event';

export type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
export type Unlisten = () => void;
export type Listen = <T>(event: string, handler: (event: { payload: T }) => void) => Promise<Unlisten>;

export interface TauriClient {
  invoke: Invoke;
  listen: Listen;
}

export function createTauriClient(): TauriClient {
  const invoke: Invoke = (command, args) => tauriInvoke(command, args);
  const listen: Listen = (event, handler) => tauriListen(event, handler);

  return { invoke, listen };
}
