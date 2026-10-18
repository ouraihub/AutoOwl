<script lang="ts">
  import type { Component } from 'svelte';
  import type { PlayMode, Song } from '$lib/domain/contracts';
  import { createTauriClient, type Unlisten } from '$lib/services/tauri-client';
  import { formatDuration } from '$lib/format';
  import { playerService } from '$lib/services/user/player-service';
  import { historyService } from '$lib/services/user/history-service';
  import { playerStore } from '$lib/stores/player.svelte';
  import { downloadsStore } from '$lib/stores/downloads.svelte';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { notify } from '$lib/stores/toast';
  import SourceBadge, { sourceName } from '$lib/components/music/SourceBadge.svelte';
  import LyricsPanel from '$lib/components/music/LyricsPanel.svelte';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Slider } from '$lib/components/ui/slider/index.js';
  import { Separator } from '$lib/components/ui/separator/index.js';
  import * as Sheet from '$lib/components/ui/sheet/index.js';
  import PlayIcon from '@lucide/svelte/icons/play';
  import PauseIcon from '@lucide/svelte/icons/pause';
  import SkipBackIcon from '@lucide/svelte/icons/skip-back';
  import SkipForwardIcon from '@lucide/svelte/icons/skip-forward';
  import ListOrderedIcon from '@lucide/svelte/icons/list-ordered';
  import ShuffleIcon from '@lucide/svelte/icons/shuffle';
  import Repeat1Icon from '@lucide/svelte/icons/repeat-1';
  import ListMusicIcon from '@lucide/svelte/icons/list-music';
  import MicVocalIcon from '@lucide/svelte/icons/mic-vocal';
  import Volume2Icon from '@lucide/svelte/icons/volume-2';
  import VolumeXIcon from '@lucide/svelte/icons/volume-x';
  import MusicIcon from '@lucide/svelte/icons/music';
  import TrashIcon from '@lucide/svelte/icons/trash';
  import XIcon from '@lucide/svelte/icons/x';
  import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';
  import ReplaceIcon from '@lucide/svelte/icons/replace';

  type ModeMeta = { label: string; icon: Component<{ class?: string }> };

  /** Icon + Chinese label per play mode; the button cycles through these. */
  const MODE_META: Record<PlayMode, ModeMeta> = {
    sequential: { label: '顺序播放', icon: ListOrderedIcon },
    random: { label: '随机播放', icon: ShuffleIcon },
    singleLoop: { label: '单曲循环', icon: Repeat1Icon }
  };

  /** Comparable track identity, so effects can key off the song rather than object identity. */
  function keyOf(song: Song | undefined): string {
    return song === undefined ? '' : `${song.ref.source}/${song.ref.scope}/${song.ref.siteId}`;
  }

  function artistsOf(song: Song): string {
    return song.artists.map((artist) => artist.name).join('、');
  }

  let audio = $state<HTMLAudioElement | null>(null);
  let queueOpen = $state(false);
  let lyricsOpen = $state(false);

  /** Set while the user drags the seek thumb, so incoming ticks cannot yank it back. */
  let seeking = $state(false);
  let seekDraft = $state(0);
  /** Non-null only mid-drag on the volume slider. */
  let volumeDraft = $state<number | null>(null);
  /** Key of the song whose cover failed to load, so the fallback shows instead. */
  let coverError = $state('');

  const current = $derived(playerStore.current);
  const currentKey = $derived(keyOf(current));
  const lyricsEnabled = $derived(settingsStore.settings?.disableFloatingLyrics !== true);
  const modeMeta = $derived(MODE_META[playerStore.mode]);
  const ModeIcon = $derived(modeMeta.icon);
  const seekMax = $derived(playerStore.durationMs > 0 ? playerStore.durationMs : 0);
  const seekValue = $derived(
    seeking ? seekDraft : Math.min(Math.max(playerStore.positionMs, 0), Math.max(seekMax, 0))
  );
  const volumeValue = $derived(volumeDraft ?? playerStore.volume);

  /** When playback came from a different platform than requested, say so, so the user knows why. */
  const switchNotice = $derived.by(() => {
    const resolution = playerStore.resolution;
    if (resolution === null || !resolution.switched) return null;
    return { requested: sourceName(resolution.requested), servedBy: sourceName(resolution.servedBy) };
  });

  const client = createTauriClient();
  type McpPlayerEvent =
    | { action: 'play'; song: Song }
    | { action: 'pause' | 'resume' | 'next' | 'previous' };

  function runMcp(event: McpPlayerEvent): Promise<void> {
    switch (event.action) {
      case 'play':
        return playerStore.playSong(event.song);
      case 'pause':
        return playerStore.pause();
      case 'resume':
        return playerStore.play();
      case 'next':
        return playerStore.next();
      case 'previous':
        return playerStore.previous();
    }
  }

  /**
   * Mount-time wiring. This lives here, not in `+layout.svelte`, because that file
   * is frozen: PlayerBar is rendered exactly once (the shell's `player` snippet),
   * which makes it the only place that can own a single `<audio>` element and a
   * single pair of event subscriptions.
   */
  $effect(() => {
    const element = audio;
    if (element === null) return;

    playerService.attachAudio(element);
    element.volume = playerStore.volume;

    let detachPlayer: Unlisten | null = null;
    let detachDownloads: Unlisten | null = null;
    let detachMcp: Unlisten | null = null;
    let disposed = false;

    void (async () => {
      const [player, downloads, mcp] = await Promise.all([
        playerStore.attach(),
        downloadsStore.attach(),
        // AI clients (MCP / CLI) drive the same store a click would, so the UI,
        // history and audio element stay in one code path.
        client.listen<McpPlayerEvent>('mcp:player', (event) => void runMcp(event.payload))
      ]);
      if (disposed) {
        player();
        downloads();
        mcp();
        return;
      }
      detachPlayer = player;
      detachDownloads = downloads;
      detachMcp = mcp;
    })();

    // Adopt whatever the backend already holds, so a reload restores the transport.
    void playerStore.sync();
    void downloadsStore.load();
    void settingsStore.load();

    return () => {
      disposed = true;
      detachPlayer?.();
      detachDownloads?.();
      detachMcp?.();
    };
  });

  /** Point the element at each newly resolved stream exactly once. */
  let appliedUrl = '';
  $effect(() => {
    const url = playerStore.resolution?.resource.url;
    if (url === undefined || url === appliedUrl) return;
    appliedUrl = url;
    playerService.setSourceUrl(url);
  });

  /**
   * Record one history entry per song start. Keyed on track identity and gated on
   * `status === 'playing'`, so position ticks (which touch neither field) never
   * re-run this, and pause/resume of the same track does not double-record.
   */
  let recordedKey = '';
  $effect(() => {
    const song = playerStore.current;
    if (song === undefined || playerStore.status !== 'playing') return;

    const key = keyOf(song);
    if (key === recordedKey) return;
    recordedKey = key;
    // History is best-effort; a failed write must not interrupt playback.
    void historyService.record(song).catch(() => undefined);
  });

  function reportFrom(element: HTMLAudioElement): void {
    const duration = element.duration;
    const durationMs = Number.isFinite(duration) ? Math.round(duration * 1000) : undefined;
    playerStore.reportPosition(Math.round(element.currentTime * 1000), durationMs);
  }

  function onTimeUpdate(event: Event): void {
    if (seeking) return;
    reportFrom(event.currentTarget as HTMLAudioElement);
  }

  /**
   * The first point at which the real container duration is known. Also the only
   * place it is pushed to the backend: `timeupdate` fires ~4x/sec and would turn
   * one report per track into an IPC flood.
   */
  function onLoadedMetadata(event: Event): void {
    const element = event.currentTarget as HTMLAudioElement;
    reportFrom(element);
    if (Number.isFinite(element.duration)) {
      void playerStore.reportDuration(Math.round(element.duration * 1000));
    }
  }

  function onAudioError(): void {
    notify.error(
      '播放失败',
      current === undefined ? '音频加载出错' : `无法播放《${current.title}》，可尝试切换音源`
    );
  }

  function onSeekChange(value: number): void {
    seeking = true;
    seekDraft = value;
  }

  function onSeekCommit(value: number): void {
    seeking = false;
    void playerStore.seek(value);
  }

  function onVolumeChange(value: number): void {
    volumeDraft = value;
    // Applied straight to the element so the change is audible before the round-trip.
    if (audio !== null) audio.volume = Math.min(1, Math.max(0, value));
  }

  function onVolumeCommit(value: number): void {
    volumeDraft = null;
    void playerStore.setVolume(value);
  }

  /** Jump to a queue position: realign the backend index, then resolve and play. */
  async function jumpTo(index: number): Promise<void> {
    const song = playerStore.queue[index];
    if (song === undefined) return;
    await playerStore.setQueue(playerStore.queue, index);
    await playerStore.playSong(song);
  }
</script>

<!--
  The single `<audio>` element for the whole app. Created in markup (never
  `new Audio()` at module scope) so nothing touches `window` during prerender.
-->
<audio
  bind:this={audio}
  class="hidden"
  preload="metadata"
  ontimeupdate={onTimeUpdate}
  onloadedmetadata={onLoadedMetadata}
  onerror={onAudioError}
  onended={() => void playerStore.next()}
></audio>

<div class="flex h-full w-full items-center gap-4 px-4">
  <div class="flex min-w-0 flex-1 items-center gap-3">
    <div
      class="flex size-12 shrink-0 items-center justify-center overflow-hidden rounded-md border border-border bg-muted"
    >
      {#if current?.coverUrl && coverError !== currentKey}
        <img
          src={current.coverUrl}
          alt={`《${current.title}》封面`}
          class="size-full object-cover"
          onerror={() => (coverError = currentKey)}
        />
      {:else}
        <MusicIcon class="size-5 text-muted-foreground" />
      {/if}
    </div>

    <div class="min-w-0 flex-1">
      {#if current === undefined}
        <p class="truncate text-sm text-muted-foreground">未在播放</p>
        <p class="truncate text-xs text-muted-foreground">选择歌曲开始播放</p>
      {:else}
        <div class="flex min-w-0 items-center gap-2">
          <p class="truncate text-sm font-medium">{current.title}</p>
          <SourceBadge source={current.ref.source} size="sm" class="shrink-0" />
        </div>
        <p class="truncate text-xs text-muted-foreground">{artistsOf(current) || '未知歌手'}</p>
      {/if}

      {#if switchNotice !== null}
        <p class="flex min-w-0 items-center gap-1 truncate text-xs text-muted-foreground">
          <span class="truncate">{switchNotice.requested}</span>
          <ArrowRightIcon class="size-3 shrink-0" />
          <span class="truncate text-primary">已自动切换到 {switchNotice.servedBy}</span>
        </p>
      {/if}
    </div>
  </div>

  <div class="flex w-full max-w-md shrink-0 flex-col items-center gap-1">
    <div class="flex items-center gap-1">
      <Button
        variant="ghost"
        size="icon"
        aria-label="上一首"
        disabled={!playerStore.canSkip}
        onclick={() => void playerStore.previous()}
      >
        <SkipBackIcon class="size-4" />
      </Button>

      <Button
        variant="default"
        size="icon"
        class="size-10 rounded-full"
        aria-label={playerStore.isPlaying ? '暂停' : '播放'}
        disabled={current === undefined}
        onclick={() => void playerStore.toggle()}
      >
        {#if playerStore.isPlaying}
          <PauseIcon class="size-5" />
        {:else}
          <PlayIcon class="size-5" />
        {/if}
      </Button>

      <Button
        variant="ghost"
        size="icon"
        aria-label="下一首"
        disabled={!playerStore.canSkip}
        onclick={() => void playerStore.next()}
      >
        <SkipForwardIcon class="size-4" />
      </Button>

      <Button
        variant="ghost"
        size="icon"
        aria-label="换源播放"
        title="在其他音源上重新解析这首歌"
        disabled={current === undefined || playerStore.loading}
        onclick={() => void playerStore.switchSource()}
      >
        <ReplaceIcon class="size-4" />
      </Button>
    </div>

    <div class="flex w-full items-center gap-2">
      <span class="w-10 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
        {formatDuration(playerStore.positionMs, '00:00')}
      </span>
      <Slider
        type="single"
        value={seekValue}
        min={0}
        max={seekMax > 0 ? seekMax : 1}
        step={1000}
        disabled={current === undefined || seekMax === 0}
        aria-label="播放进度"
        aria-valuetext={`${formatDuration(seekValue, '00:00')} / ${formatDuration(seekMax)}`}
        onValueChange={onSeekChange}
        onValueCommit={onSeekCommit}
        class="min-w-0 flex-1"
      />
      <span class="w-10 shrink-0 text-xs tabular-nums text-muted-foreground">
        {formatDuration(seekMax > 0 ? seekMax : current?.durationMs)}
      </span>
    </div>
  </div>

  <div class="flex flex-1 items-center justify-end gap-1">
    <Button
      variant="ghost"
      size="icon"
      aria-label={`播放模式：${modeMeta.label}，点击切换`}
      title={modeMeta.label}
      onclick={() => void playerStore.cycleMode()}
    >
      <ModeIcon class="size-4" />
    </Button>

    <Sheet.Root bind:open={queueOpen}>
      <Sheet.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon"
            aria-label={`播放队列，共 ${playerStore.queue.length} 首`}
          >
            <ListMusicIcon class="size-4" />
          </Button>
        {/snippet}
      </Sheet.Trigger>

      <Sheet.Content side="right" class="flex w-full flex-col gap-0 sm:max-w-md">
        <Sheet.Header>
          <Sheet.Title>播放队列</Sheet.Title>
          <Sheet.Description>共 {playerStore.queue.length} 首，点击任意歌曲立即播放。</Sheet.Description>
        </Sheet.Header>

        <Separator />

        {#if playerStore.queue.length === 0}
          <p class="px-4 py-8 text-center text-sm text-muted-foreground">队列为空。</p>
        {:else}
          <ol class="min-h-0 flex-1 overflow-y-auto py-2">
            {#each playerStore.queue as song, index (`${keyOf(song)}:${index}`)}
              <li
                class="group flex items-center gap-2 px-4 py-2 {index === playerStore.currentIndex
                  ? 'bg-accent'
                  : ''}"
              >
                <button
                  type="button"
                  class="flex min-w-0 flex-1 items-center gap-3 text-left focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  aria-label={`播放 ${song.title}`}
                  aria-current={index === playerStore.currentIndex ? 'true' : undefined}
                  onclick={() => void jumpTo(index)}
                >
                  <span class="w-6 shrink-0 text-xs tabular-nums text-muted-foreground">
                    {index + 1}
                  </span>
                  <span class="min-w-0 flex-1">
                    <span class="block truncate text-sm">{song.title}</span>
                    <span class="block truncate text-xs text-muted-foreground">
                      {artistsOf(song) || '未知歌手'}
                    </span>
                  </span>
                  <span class="shrink-0 text-xs tabular-nums text-muted-foreground">
                    {formatDuration(song.durationMs, '')}
                  </span>
                </button>

                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="shrink-0 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
                  aria-label={`从队列移除 ${song.title}`}
                  onclick={() => void playerStore.removeFromQueue(index)}
                >
                  <XIcon class="size-4" />
                </Button>
              </li>
            {/each}
          </ol>

          <Separator />
          <div class="flex justify-end p-4">
            <Button
              variant="destructive"
              size="sm"
              onclick={() => void playerStore.clearQueue()}
            >
              <TrashIcon class="size-4" />
              清空队列
            </Button>
          </div>
        {/if}
      </Sheet.Content>
    </Sheet.Root>

    <Button
      variant="ghost"
      size="icon"
      aria-label={lyricsEnabled ? '歌词' : '歌词已在设置中关闭'}
      title="歌词"
      disabled={!lyricsEnabled || current === undefined}
      onclick={() => (lyricsOpen = !lyricsOpen)}
    >
      <MicVocalIcon class="size-4" />
    </Button>

    {#if volumeValue === 0}
      <VolumeXIcon class="size-4 shrink-0 text-muted-foreground" />
    {:else}
      <Volume2Icon class="size-4 shrink-0 text-muted-foreground" />
    {/if}

    <Slider
      type="single"
      value={volumeValue}
      min={0}
      max={1}
      step={0.01}
      aria-label="音量"
      aria-valuetext={`${Math.round(volumeValue * 100)}%`}
      onValueChange={onVolumeChange}
      onValueCommit={onVolumeCommit}
      class="w-24 shrink-0"
    />
  </div>
</div>

<Sheet.Root bind:open={lyricsOpen}>
  <Sheet.Content side="right" class="flex w-full flex-col gap-3 sm:max-w-md">
    <Sheet.Header>
      <Sheet.Title>歌词</Sheet.Title>
      <Sheet.Description>
        {current === undefined ? '播放歌曲后显示歌词。' : current.title}
      </Sheet.Description>
    </Sheet.Header>
    <Separator />
    <div class="flex min-h-0 flex-1 flex-col overflow-hidden px-4 pb-4">
      <LyricsPanel song={current} />
    </div>
  </Sheet.Content>
</Sheet.Root>
