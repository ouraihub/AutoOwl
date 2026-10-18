import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  DownloadProgressEvent,
  DownloadState,
  DownloadTask,
  Song
} from '$lib/domain/contracts';
import { DownloadsStore } from './downloads.svelte';

/** Handlers captured from the mocked service, invoked to simulate Rust events. */
const listeners = {
  changed: [] as Array<(task: DownloadTask) => void>,
  progress: [] as Array<(progress: DownloadProgressEvent) => void>
};

let listResult: DownloadTask[] = [];

vi.mock('$lib/services/user/download-service', () => ({
  downloadService: {
    listTasks: () => Promise.resolve(listResult),
    create: vi.fn(),
    createBatch: vi.fn(),
    cancel: vi.fn(),
    retry: vi.fn(),
    getTask: vi.fn(),
    openFolder: vi.fn(),
    onChanged: (handler: (task: DownloadTask) => void) => {
      listeners.changed.push(handler);
      return Promise.resolve(() => {
        listeners.changed = listeners.changed.filter((h) => h !== handler);
      });
    },
    onProgress: (handler: (progress: DownloadProgressEvent) => void) => {
      listeners.progress.push(handler);
      return Promise.resolve(() => {
        listeners.progress = listeners.progress.filter((h) => h !== handler);
      });
    }
  }
}));

vi.mock('$lib/services/errors', () => ({
  toAppError: (cause: unknown) => ({
    code: 'unknown',
    message: String(cause),
    retryable: false,
    operationId: 'test'
  })
}));

function song(title: string): Song {
  return {
    ref: { source: 'netease', scope: 'song', siteId: title },
    title,
    artists: [{ name: '测试歌手' }]
  };
}

function task(id: string, state: DownloadState, extra: Partial<DownloadTask> = {}): DownloadTask {
  return {
    id,
    song: song(id),
    targetPath: `D:/music/${id}.mp3`,
    state,
    progress: 0,
    downloadedBytes: 0,
    skipped: false,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    ...extra
  };
}

/** Load a store seeded with `rows` and subscribed to the fake event stream. */
async function attachedStore(rows: DownloadTask[]) {
  listResult = rows;
  const store = new DownloadsStore();
  await store.load();
  const detach = await store.attach();
  return { store, detach };
}

const emitChanged = (next: DownloadTask) => listeners.changed.forEach((h) => h(next));
const emitProgress = (next: DownloadProgressEvent) => listeners.progress.forEach((h) => h(next));

beforeEach(() => {
  listeners.changed = [];
  listeners.progress = [];
  listResult = [];
});

describe('download:progress', () => {
  it('patches byte counters without clobbering state', async () => {
    // Seeded as `processing` on purpose: a store that stamped any lifecycle
    // state onto a progress frame would overwrite it with something else.
    const { store } = await attachedStore([
      task('a', 'processing', { progress: 0.1, downloadedBytes: 100 })
    ]);

    emitProgress({
      id: 'a',
      downloadedBytes: 5_000,
      totalBytes: 10_000,
      progress: 0.5,
      speedBytesPerSecond: 1_024
    });

    const row = store.tasks[0];
    expect(row.downloadedBytes).toBe(5_000);
    expect(row.totalBytes).toBe(10_000);
    expect(row.progress).toBe(0.5);
    expect(row.speedBytesPerSecond).toBe(1_024);
    // The invariant: a progress frame must never move the lifecycle state.
    expect(row.state).toBe('processing');
  });

  it('does not let a late progress frame revive a completed task', async () => {
    const { store } = await attachedStore([task('a', 'downloading')]);

    emitChanged(task('a', 'completed', { progress: 1, downloadedBytes: 10_000 }));
    emitProgress({ id: 'a', downloadedBytes: 9_000, totalBytes: 10_000, progress: 0.9 });

    expect(store.tasks).toHaveLength(1);
    expect(store.tasks[0].state).toBe('completed');
    expect(store.tasks[0].downloadedBytes).toBe(9_000);
    expect(store.completedCount).toBe(1);
    expect(store.activeCount).toBe(0);
  });

  it('ignores progress for an unknown id', async () => {
    const { store } = await attachedStore([task('a', 'downloading')]);

    emitProgress({ id: 'ghost', downloadedBytes: 42, progress: 0.42 });

    expect(store.tasks).toHaveLength(1);
    expect(store.tasks[0].downloadedBytes).toBe(0);
  });

  it('keeps a known total when a frame omits it', async () => {
    const { store } = await attachedStore([
      task('a', 'downloading', { totalBytes: 10_000, speedBytesPerSecond: 500 })
    ]);

    emitProgress({ id: 'a', downloadedBytes: 2_000, progress: 0.2 });

    expect(store.tasks[0].totalBytes).toBe(10_000);
    expect(store.tasks[0].downloadedBytes).toBe(2_000);
  });
});

describe('download:changed', () => {
  it('replaces an existing row instead of appending a duplicate', async () => {
    const { store } = await attachedStore([task('a', 'queued'), task('b', 'queued')]);

    emitChanged(task('a', 'downloading', { progress: 0.25 }));

    expect(store.tasks).toHaveLength(2);
    expect(store.tasks.filter((t) => t.id === 'a')).toHaveLength(1);
    expect(store.tasks[0].state).toBe('downloading');
    expect(store.tasks[0].progress).toBe(0.25);
    // The replaced row keeps its slot, so the list does not reorder under the cursor.
    expect(store.tasks.map((t) => t.id)).toEqual(['a', 'b']);
  });

  it('appends a task whose id is new', async () => {
    const { store } = await attachedStore([task('a', 'queued')]);

    emitChanged(task('c', 'resolving'));

    expect(store.tasks.map((t) => t.id)).toEqual(['a', 'c']);
  });

  it('stops reconciling after detach', async () => {
    const { store, detach } = await attachedStore([task('a', 'queued')]);
    detach();

    emitChanged(task('a', 'completed'));
    emitProgress({ id: 'a', downloadedBytes: 99, progress: 0.99 });

    expect(store.tasks[0].state).toBe('queued');
    expect(store.tasks[0].downloadedBytes).toBe(0);
  });
});

describe('counters', () => {
  it('counts exactly the four in-flight states as active', async () => {
    const { store } = await attachedStore([
      task('q', 'queued'),
      task('r', 'resolving'),
      task('d', 'downloading'),
      task('p', 'processing'),
      task('done', 'completed'),
      task('fail', 'failed'),
      task('cr', 'cancelRequested'),
      task('cx', 'cancelled')
    ]);

    expect(store.activeCount).toBe(4);
    expect(store.completedCount).toBe(1);
    expect(store.failedCount).toBe(1);
  });

  it('tracks counters across the whole lifecycle of one task', async () => {
    const { store } = await attachedStore([task('a', 'queued')]);
    expect(store.activeCount).toBe(1);

    emitChanged(task('a', 'downloading'));
    expect(store.activeCount).toBe(1);

    emitChanged(task('a', 'failed'));
    expect(store.activeCount).toBe(0);
    expect(store.failedCount).toBe(1);

    emitChanged(task('a', 'completed'));
    expect(store.failedCount).toBe(0);
    expect(store.completedCount).toBe(1);
  });
});
