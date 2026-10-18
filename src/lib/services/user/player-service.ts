import type {
  PlayMode,
  PlayerState,
  ResolvedPlayback,
  Song,
  SourceSwitch
} from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

/**
 * The Rust player owns queue/mode/position state; the browser owns the actual
 * audio pipeline. A component hands the single `<audio>` element over via
 * `attachAudio`, and the mutating commands below mirror their effect onto it so
 * the element never drifts from the state the backend reports.
 *
 * Never constructed here: module scope also runs during prerender, where there
 * is no `window`.
 */
let audio: HTMLAudioElement | null = null;

/** Hand the real element over. Re-attaching replaces the previous one. */
export function attachAudio(element: HTMLAudioElement): void {
  audio = element;
}

/**
 * Point the element at a resolved stream URL and load it. Remote URLs go through
 * the `stream` protocol so Rust can add the source's Referer/User-Agent/Cookie,
 * which `<audio>` cannot send (bilibili and migu CDNs reject requests without them).
 */
export function setSourceUrl(url: string, source: string): void {
  if (typeof window === 'undefined' || audio === null) return;
  audio.src = /^https?:\/\//i.test(url)
    ? `http://stream.localhost/?${new URLSearchParams({ source, url })}`
    : url;
  audio.load();
}

export const playerService = {
  attachAudio,
  setSourceUrl,

  /** Probe sources for a playable stream, following an automatic switch. */
  resolvePlayback(song: Song): Promise<ResolvedPlayback> {
    return client.invoke<ResolvedPlayback>('resolve_playback', { song });
  },

  /**
   * User-initiated "换源". Never called automatically: a download or playback
   * failure surfaces the source's own error, and only an explicit click probes
   * other sources.
   */
  switchSource(song: Song): Promise<SourceSwitch> {
    return client.invoke<SourceSwitch>('switch_source', { song });
  },

  load(song: Song): Promise<PlayerState> {
    return client.invoke<PlayerState>('player_load', { song });
  },

  async play(): Promise<PlayerState> {
    const state = await client.invoke<PlayerState>('player_play');
    await audio?.play();
    return state;
  },

  async pause(): Promise<PlayerState> {
    const state = await client.invoke<PlayerState>('player_pause');
    audio?.pause();
    return state;
  },

  next(): Promise<PlayerState> {
    return client.invoke<PlayerState>('player_next');
  },

  previous(): Promise<PlayerState> {
    return client.invoke<PlayerState>('player_previous');
  },

  async seek(positionMs: number): Promise<PlayerState> {
    const state = await client.invoke<PlayerState>('player_seek', { positionMs });
    if (audio !== null) audio.currentTime = state.positionMs / 1000;
    return state;
  },

  /**
   * Hand the decoded container duration to the backend, which needs it to clamp
   * seeks. Call once per track from `loadedmetadata`, never from `timeupdate`.
   */
  reportDuration(durationMs: number): Promise<PlayerState> {
    return client.invoke<PlayerState>('player_report_duration', { durationMs });
  },

  setMode(mode: PlayMode): Promise<PlayerState> {
    return client.invoke<PlayerState>('player_set_mode', { mode });
  },

  getState(): Promise<PlayerState> {
    return client.invoke<PlayerState>('player_get_state');
  },

  /** `volume` is 0..1, matching both the Rust `f32` and the element. */
  async setVolume(volume: number): Promise<PlayerState> {
    const state = await client.invoke<PlayerState>('player_set_volume', { volume });
    if (audio !== null) audio.volume = state.volume;
    return state;
  },

  setQueue(songs: Song[], startIndex: number): Promise<PlayerState> {
    return client.invoke<PlayerState>('player_set_queue', { songs, startIndex });
  },

  removeFromQueue(index: number): Promise<PlayerState> {
    return client.invoke<PlayerState>('player_remove_from_queue', { index });
  },

  clearQueue(): Promise<PlayerState> {
    return client.invoke<PlayerState>('player_clear_queue');
  },

  moveInQueue(from: number, to: number): Promise<PlayerState> {
    return client.invoke<PlayerState>('player_move_in_queue', { from, to });
  }
};
