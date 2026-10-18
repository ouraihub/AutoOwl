import type { AppError, AppSettings } from '$lib/domain/contracts';
import { settingsService } from '$lib/services/admin/settings-service';
import { toAppError } from '$lib/services/errors';

/** Application settings mirror, loaded once and written back on save. */
export class SettingsStore {
  settings = $state<AppSettings | null>(null);
  loading = $state(false);
  error = $state<AppError | null>(null);

  readonly loaded = $derived(this.settings !== null);

  async load(): Promise<void> {
    this.loading = true;
    try {
      this.settings = await settingsService.get();
      this.error = null;
    } catch (cause) {
      this.error = toAppError(cause);
    } finally {
      this.loading = false;
    }
  }

  /**
   * Persist settings and adopt whatever the backend echoes back, so normalized
   * values (e.g. a corrected download directory) land in the UI.
   */
  async save(next: AppSettings): Promise<void> {
    this.loading = true;
    try {
      this.settings = await settingsService.update(next);
      this.error = null;
    } catch (cause) {
      this.error = toAppError(cause);
    } finally {
      this.loading = false;
    }
  }

  clearError(): void {
    this.error = null;
  }
}

export const settingsStore = new SettingsStore();
