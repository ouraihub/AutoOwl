import type {
  BatchDownloadReport,
  DownloadChangedEvent,
  DownloadProgressEvent,
  DownloadRecordsPage,
  DownloadTask,
  Song
} from '$lib/domain/contracts';
import { createTauriClient, type Unlisten } from '$lib/services/tauri-client';

const client = createTauriClient();

export const downloadService = {
  create(song: Song): Promise<DownloadTask> {
    return client.invoke<DownloadTask>('create_download', { song });
  },

  /** Duplicates are reported as skipped items, not as a rejection. */
  createBatch(songs: Song[]): Promise<BatchDownloadReport> {
    return client.invoke<BatchDownloadReport>('create_batch_download', { songs });
  },

  cancel(taskId: string): Promise<void> {
    return client.invoke<void>('cancel_download', { taskId });
  },

  retry(taskId: string): Promise<DownloadTask> {
    return client.invoke<DownloadTask>('retry_download', { taskId });
  },

  getTask(taskId: string): Promise<DownloadTask> {
    return client.invoke<DownloadTask>('get_download_task', { taskId });
  },

  listTasks(): Promise<DownloadTask[]> {
    return client.invoke<DownloadTask[]>('list_download_tasks');
  },

  /** Persisted download history, newest first. An overshooting page clamps down. */
  listRecords(page: number, pageSize: number): Promise<DownloadRecordsPage> {
    return client.invoke<DownloadRecordsPage>('list_download_records', { page, pageSize });
  },

  /** Wipes the records table; returns how many rows were removed. Irreversible. */
  clearRecords(): Promise<number> {
    return client.invoke<number>('clear_download_records');
  },

  /** Reveals one task's file, or the download root when `taskId` is omitted. Returns the opened path. */
  openFolder(taskId?: string): Promise<string> {
    return client.invoke<string>('open_download_folder', { taskId });
  },

  /** Fires on every task state transition. */
  onChanged(handler: (task: DownloadTask) => void): Promise<Unlisten> {
    return client.listen<DownloadChangedEvent>('download:changed', (event) =>
      handler(event.payload.task)
    );
  },

  /** Fires while bytes move; high frequency, so throttle in the consumer if needed. */
  onProgress(handler: (progress: DownloadProgressEvent) => void): Promise<Unlisten> {
    return client.listen<DownloadProgressEvent>('download:progress', (event) =>
      handler(event.payload)
    );
  }
};
