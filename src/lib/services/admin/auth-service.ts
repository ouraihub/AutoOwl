import type {
  AuthStatus,
  QrLoginResult,
  QrLoginSession,
  SourceId
} from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

/** Per-source cookie credentials. Each mutation returns that source's new status. */
export const authService = {
  getStatus(source: SourceId): Promise<AuthStatus> {
    return client.invoke<AuthStatus>('get_auth_status', { source });
  },

  saveCookie(source: SourceId, cookie: string): Promise<AuthStatus> {
    return client.invoke<AuthStatus>('save_source_cookie', { source, cookie });
  },

  clearCookie(source: SourceId): Promise<AuthStatus> {
    return client.invoke<AuthStatus>('clear_source_cookie', { source });
  },

  listStatus(): Promise<AuthStatus[]> {
    return client.invoke<AuthStatus[]>('list_auth_status');
  },

  /** Opens a QR session; `imageUrl` is a ready-to-render data URL. */
  createQrLogin(source: SourceId): Promise<QrLoginSession> {
    return client.invoke<QrLoginSession>('qr_login_create', { source });
  },

  /**
   * Polls a QR session. The backend persists the cookie itself once the status
   * is `confirmed`, so callers only need to refresh the displayed status.
   */
  checkQrLogin(source: SourceId, key: string): Promise<QrLoginResult> {
    return client.invoke<QrLoginResult>('qr_login_check', { source, key });
  }
};
