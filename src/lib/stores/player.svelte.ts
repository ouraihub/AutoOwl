import type {
  AppError,
  PlayerState,
  PlayerStatus,
  PlayMode,
  ResolvedPlayback,
  Song
} from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';
import { toAppError } from '$lib/services/errors';
import { playerService } from '$lib/services/user/player-service';

const client = createTauriClient();

/** Mode cycling order used by the mode button: 顺序 -> 随机 -> 单曲循环. */
const MODE_CYCLE: readonly PlayMode[] = ['sequential', 'random', 'singleLoop'];

/** Stable identity for a song, so per-track side effects can dedupe on it. */
function trackKey(song: Song | undefined): string {
  return song === undefined ? '' : `${song.ref.source}/${song.ref.scope}/${song.ref.siteId}`;
}

/**
 * Authoritative UI mirror of the backend player.
 *
 * Local runes drive rendering so the transport stays responsive between IPC
 * round-trips; every backend reply and every `player:state` event is reconciled
 * back into these fields.
 */
export class PlayerStore {
  status = $state<PlayerStatus>('idle');
  current = $state<Song | undefined>(undefined);
  positionMs = $state(0);
  durationMs = $state(0);
  volume = $state(1);
  mode = $state<PlayMode>('sequential');
  queue = $state<Song[]>([]);
  currentIndex = $state<number | null>(null);

  loading = $state(false);
  error = $state<AppError | null>(null);

  /** Last playback resolution, so the UI can flag an automatic source switch. */
  resolution = $state<ResolvedPlayback | null>(null);

  /**
   * Track whose duration has already been reported to the backend. Plain field,
   * not `$state`: it gates an IPC side effect and must never drive rendering.
   */
  private reportedDurationKey = '';

  /** 0..1 position within the current track; 0 when the duration is unknown. */
  readonly progressRatio = $derived(
    this.durationMs > 0 ? Math.min(1, Math.max(0, this.positionMs / this.durationMs)) : 0
  );

  readonly isPlaying = $derived(this.status === 'playing');
  readonly canSkip = $derived(this.queue.length > 1);

  /** Snapshot of the mirrored fields as a plain `PlayerState`. */
  get state(): PlayerState {
    return {
      status: this.status,
      current: this.current,
      positionMs: this.positionMs,
      durationMs: this.durationMs,
      volume: this.volume,
      mode: this.mode,
      queue: this.queue,
      currentIndex: this.currentIndex
    };
  }

  /** Reconcile a backend state snapshot into the local runes. */
  apply(next: PlayerState): void {
    this.status = next.status;
    this.current = next.current;
    this.positionMs = next.positionMs;
    this.durationMs = next.durationMs;
    this.volume = next.volume;
    this.mode = next.mode;
    this.queue = next.queue;
    this.currentIndex = next.currentIndex;
  }

  /**
   * Subscribe to `player:state` and reconcile every backend mutation.
   * Returns a detach function.
   */
  attach(): Promise<() => void> {
    return client.listen<PlayerState>('player:state', (event) => this.apply(event.payload));
  }

  /** Pull the authoritative backend state. */
  async sync(): Promise<void> {
    await this.run(() => playerService.getState());
  }

  /**
   * Resolve a stream URL, point the audio element at it, then load and play.
   * Recording the play into history is the caller's job, not this store's.
   */
  async playSong(song: Song): Promise<void> {
    this.loading = true;
    try {
      const resolved = await playerService.resolvePlayback(song);
      this.resolution = resolved;
      playerService.setSourceUrl(resolved.resource.url, resolved.resource.song.source);
      // `load` re-seeds the backend duration from song metadata, which is often
      // absent. Re-arm so the decoded duration is reported again, even when the
      // same track is replayed.
      this.reportedDurationKey = '';
      await playerService.load(song);
      this.apply(await playerService.play());
      this.error = null;
    } catch (cause) {
      this.status = 'failed';
      this.error = toAppError(cause);
    } finally {
      this.loading = false;
    }
  }

  /**
   * Re-resolve the current track on a different source, then keep playing.
   * Only ever driven by an explicit user action; a playback failure alone must
   * surface the failing source's error instead of silently hopping sources.
   */
  async switchSource(): Promise<void> {
    const song = this.current;
    if (song === undefined) return;
    this.loading = true;
    try {
      const outcome = await playerService.switchSource(song);
      playerService.setSourceUrl(outcome.resource.url, outcome.resource.song.source);
      this.reportedDurationKey = '';
      // The alternative is a different platform's recording of the same track,
      // so the backend must be loaded with the new ref while the user-visible
      // metadata stays the one they searched for.
      await playerService.load({ ...song, ref: outcome.resource.song });
      this.apply(await playerService.play());
      this.error = null;
    } catch (cause) {
      this.error = toAppError(cause);
    } finally {
      this.loading = false;
    }
  }

  /** Play/pause on the current track. */
  async toggle(): Promise<void> {
    await this.run(() => (this.status === 'playing' ? playerService.pause() : playerService.play()));
  }

  async play(): Promise<void> {
    await this.run(() => playerService.play());
  }

  async pause(): Promise<void> {
    await this.run(() => playerService.pause());
  }

  async next(): Promise<void> {
    await this.run(() => playerService.next());
  }

  async previous(): Promise<void> {
    await this.run(() => playerService.previous());
  }

  /** Seek, updating the slider immediately so the drag does not snap back. */
  async seek(positionMs: number): Promise<void> {
    this.positionMs = positionMs;
    await this.run(() => playerService.seek(positionMs));
  }

  /** Set volume, applying it locally first for an instant response. */
  async setVolume(volume: number): Promise<void> {
    const clamped = Math.min(1, Math.max(0, volume));
    this.volume = clamped;
    await this.run(() => playerService.setVolume(clamped));
  }

  async setMode(mode: PlayMode): Promise<void> {
    await this.run(() => playerService.setMode(mode));
  }

  /** 顺序 -> 随机 -> 单曲循环 -> 顺序. */
  async cycleMode(): Promise<void> {
    const at = MODE_CYCLE.indexOf(this.mode);
    const next = MODE_CYCLE[(at + 1) % MODE_CYCLE.length];
    await this.setMode(next);
  }

  async setQueue(songs: Song[], startIndex = 0): Promise<void> {
    await this.run(() => playerService.setQueue(songs, startIndex));
  }

  async removeFromQueue(index: number): Promise<void> {
    await this.run(() => playerService.removeFromQueue(index));
  }

  async clearQueue(): Promise<void> {
    await this.run(() => playerService.clearQueue());
  }

  async moveInQueue(from: number, to: number): Promise<void> {
    await this.run(() => playerService.moveInQueue(from, to));
  }

  /** Local-only position tick driven by the audio element's `timeupdate`. */
  reportPosition(positionMs: number, durationMs?: number): void {
    this.positionMs = positionMs;
    if (durationMs !== undefined && Number.isFinite(durationMs)) this.durationMs = durationMs;
  }

  /**
   * Push a newly decoded container duration to the backend, which needs it to
   * clamp seeks. Deliberately separate from `reportPosition`: that runs on every
   * `timeupdate` (~4x/sec) and would flood IPC. Keyed on track identity so a
   * replay of the same track does not re-send, and so switching tracks re-arms.
   */
  async reportDuration(durationMs: number): Promise<void> {
    if (!Number.isFinite(durationMs) || durationMs <= 0) return;

    const key = trackKey(this.current);
    if (key === '' || key === this.reportedDurationKey) return;
    this.reportedDurationKey = key;

    this.durationMs = durationMs;
    try {
      this.apply(await playerService.reportDuration(durationMs));
    } catch {
      // Best effort: a failed report only costs seek clamping, never playback.
      // Re-arm so the next metadata event for this track can retry.
      this.reportedDurationKey = '';
    }
  }

  clearError(): void {
    this.error = null;
  }

  /**
   * Run one service call: reconcile the returned state and clear the previous
   * error on success, or normalize the failure into `error`.
   */
  private async run(call: () => Promise<PlayerState>): Promise<void> {
    this.loading = true;
    try {
      this.apply(await call());
      this.error = null;
    } catch (cause) {
      this.error = toAppError(cause);
    } finally {
      this.loading = false;
    }
  }
}

export const playerStore = new PlayerStore();
