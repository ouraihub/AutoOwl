import type {
  AppError,
  DownloadProgressEvent,
  DownloadState,
  DownloadTask,
  Song
} from '$lib/domain/contracts';
import type { Unlisten } from '$lib/services/tauri-client';
import { toAppError } from '$lib/services/errors';
import { downloadService } from '$lib/services/user/download-service';

/** States where the task is still moving and belongs in the "进行中" bucket. */
const IN_FLIGHT: readonly DownloadState[] = ['queued', 'resolving', 'downloading', 'processing'];

/**
 * Download queue mirror.
 *
 * `download:changed` carries a whole task and replaces the row; `download:progress`
 * is a high-frequency byte-counter patch that must never touch `state`, otherwise
 * a late progress frame would resurrect a finished task.
 */
export class DownloadsStore {
  tasks = $state<DownloadTask[]>([]);
  loading = $state(false);
  error = $state<AppError | null>(null);

  readonly activeCount = $derived(this.tasks.filter((t) => IN_FLIGHT.includes(t.state)).length);
  readonly completedCount = $derived(this.tasks.filter((t) => t.state === 'completed').length);
  readonly failedCount = $derived(this.tasks.filter((t) => t.state === 'failed').length);
  readonly hasTasks = $derived(this.tasks.length > 0);

  /** Replace a task by id, or append it when the id is new. */
  upsert(task: DownloadTask): void {
    const at = this.tasks.findIndex((item) => item.id === task.id);
    if (at === -1) this.tasks = [...this.tasks, task];
    else this.tasks = this.tasks.with(at, task);
  }

  /**
   * Patch only the byte counters of a live task. `state` is deliberately left
   * alone: progress frames race with `download:changed` transitions.
   */
  patchProgress(patch: DownloadProgressEvent): void {
    const at = this.tasks.findIndex((item) => item.id === patch.id);
    if (at === -1) return;

    const current = this.tasks[at];
    this.tasks = this.tasks.with(at, {
      ...current,
      downloadedBytes: patch.downloadedBytes,
      totalBytes: patch.totalBytes ?? current.totalBytes,
      progress: patch.progress,
      speedBytesPerSecond: patch.speedBytesPerSecond ?? current.speedBytesPerSecond
    });
  }

  /** Pull the full task list. */
  async load(): Promise<void> {
    this.loading = true;
    try {
      this.tasks = await downloadService.listTasks();
      this.error = null;
    } catch (cause) {
      this.error = toAppError(cause);
    } finally {
      this.loading = false;
    }
  }

  /** Subscribe to both download events; returns a single detach function. */
  async attach(): Promise<Unlisten> {
    const detachChanged = await downloadService.onChanged((task) => this.upsert(task));
    const detachProgress = await downloadService.onProgress((progress) =>
      this.patchProgress(progress)
    );

    return () => {
      detachChanged();
      detachProgress();
    };
  }

  async enqueue(song: Song): Promise<void> {
    await this.mutate(async () => {
      this.upsert(await downloadService.create(song));
    });
  }

  async enqueueBatch(songs: Song[]): Promise<void> {
    await this.mutate(async () => {
      await downloadService.createBatch(songs);
      this.tasks = await downloadService.listTasks();
    });
  }

  async cancel(id: string): Promise<void> {
    await this.mutate(() => downloadService.cancel(id));
  }

  async retry(id: string): Promise<void> {
    await this.mutate(() => downloadService.retry(id));
  }

  async openFolder(id?: string): Promise<void> {
    await this.mutate(() => downloadService.openFolder(id));
  }

  clearError(): void {
    this.error = null;
  }

  /** Run one mutation, clearing the previous error on success. */
  private async mutate(call: () => Promise<unknown>): Promise<void> {
    this.loading = true;
    try {
      await call();
      this.error = null;
    } catch (cause) {
      this.error = toAppError(cause);
    } finally {
      this.loading = false;
    }
  }
}

export const downloadsStore = new DownloadsStore();
