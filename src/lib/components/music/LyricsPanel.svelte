<script lang="ts">
  import type { AppError, LyricLine, Lyrics, Song } from '$lib/domain/contracts';
  import { toAppError } from '$lib/services/errors';
  import { lyricsService } from '$lib/services/user/lyrics-service';
  import { playerStore } from '$lib/stores/player.svelte';
  import { formatDuration } from '$lib/format';
  import * as Empty from '$lib/components/ui/empty/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import MicVocalIcon from '@lucide/svelte/icons/mic-vocal';
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';

  let { song }: { song: Song | undefined } = $props();

  let lyrics = $state<Lyrics | null>(null);
  let loading = $state(false);
  let error = $state<AppError | null>(null);

  /**
   * The song identity as a comparable string. Reloading is keyed off this rather
   * than the `Song` object: the store replaces `current` on every backend state
   * event, so an object-identity dependency would refetch on each position tick.
   */
  const songKey = $derived(
    song === undefined ? '' : `${song.ref.source}/${song.ref.scope}/${song.ref.siteId}`
  );

  /** Timed lines only; an untimed LRC still renders, just without highlighting. */
  const lines = $derived<LyricLine[]>(lyrics?.lines ?? []);
  const hasTiming = $derived(lines.some((line) => typeof line.startMs === 'number'));

  /**
   * Index of the last line whose `startMs` has been passed. `-1` while the
   * playhead sits before the first timestamp, or when the LRC carries no timing.
   */
  const activeIndex = $derived.by(() => {
    if (!hasTiming) return -1;
    const at = playerStore.positionMs;
    let found = -1;
    for (let index = 0; index < lines.length; index += 1) {
      const startMs = lines[index].startMs;
      if (typeof startMs === 'number' && startMs <= at) found = index;
      else if (typeof startMs === 'number') break;
    }
    return found;
  });

  async function load(key: string, ref: Song['ref'] | undefined): Promise<void> {
    if (key === '' || ref === undefined) {
      lyrics = null;
      error = null;
      return;
    }

    loading = true;
    try {
      const fetched = await lyricsService.get(ref);
      // A late reply for a track the user already skipped past must not land.
      if (key !== songKey) return;
      lyrics = fetched;
      error = null;
    } catch (cause) {
      if (key !== songKey) return;
      lyrics = null;
      error = toAppError(cause);
    } finally {
      if (key === songKey) loading = false;
    }
  }

  $effect(() => {
    const key = songKey;
    const ref = song?.ref;
    void load(key, ref);
  });

  let viewport = $state<HTMLElement | null>(null);

  /**
   * Keep the active line centred. `scrollTop` math is used instead of
   * `scrollIntoView` so the surrounding page never scrolls with it.
   */
  $effect(() => {
    const index = activeIndex;
    const container = viewport;
    if (container === null || index < 0) return;

    const target = container.querySelector<HTMLElement>(`[data-line="${index}"]`);
    if (target === null) return;

    container.scrollTo({
      top: target.offsetTop - container.clientHeight / 2 + target.clientHeight / 2,
      behavior: 'smooth'
    });
  });

  function retry(): void {
    void load(songKey, song?.ref);
  }
</script>

<div class="flex min-h-0 flex-1 flex-col gap-3">
  {#if song === undefined}
    <Empty.Root class="border">
      <Empty.Header>
        <Empty.Media variant="icon"><MicVocalIcon /></Empty.Media>
        <Empty.Title>暂无播放</Empty.Title>
        <Empty.Description>播放歌曲后将在此显示歌词。</Empty.Description>
      </Empty.Header>
    </Empty.Root>
  {:else if loading && lyrics === null}
    <div class="flex flex-col gap-3 px-2 py-4">
      {#each [0, 1, 2, 3, 4, 5, 6, 7] as row (row)}
        <Skeleton class="h-4" style={`width: ${[70, 55, 80, 45, 65, 75, 50, 60][row]}%`} />
      {/each}
    </div>
  {:else if error !== null}
    <Empty.Root class="border">
      <Empty.Header>
        <Empty.Media variant="icon"><CircleAlertIcon class="text-destructive" /></Empty.Media>
        <Empty.Title>歌词加载失败</Empty.Title>
        <Empty.Description>{error.message}</Empty.Description>
      </Empty.Header>
      <Empty.Content>
        <Button variant="outline" size="sm" onclick={retry}>重试</Button>
      </Empty.Content>
    </Empty.Root>
  {:else if lines.length === 0}
    <Empty.Root class="border">
      <Empty.Header>
        <Empty.Media variant="icon"><MicVocalIcon /></Empty.Media>
        <Empty.Title>纯音乐 / 无歌词</Empty.Title>
        <Empty.Description>该歌曲未提供歌词。</Empty.Description>
      </Empty.Header>
    </Empty.Root>
  {:else}
    <div
      bind:this={viewport}
      class="min-h-0 flex-1 overflow-y-auto px-2 py-4"
      role="log"
      aria-live="polite"
      aria-label="歌词"
    >
      <ol class="flex flex-col gap-3">
        {#each lines as line, index (index)}
          <li
            data-line={index}
            aria-current={index === activeIndex ? 'true' : undefined}
            class="flex items-baseline gap-3 text-center transition-colors {index === activeIndex
              ? 'text-primary font-medium'
              : 'text-muted-foreground'}"
          >
            {#if typeof line.startMs === 'number'}
              <span class="w-12 shrink-0 text-left text-xs tabular-nums opacity-60">
                {formatDuration(line.startMs, '')}
              </span>
            {/if}
            <span class="min-w-0 flex-1 text-left text-sm leading-relaxed">
              {line.text || ' '}
            </span>
          </li>
        {/each}
      </ol>
    </div>

    {#if !hasTiming}
      <p class="shrink-0 px-2 pb-2 text-xs text-muted-foreground">该歌词无时间轴，不随播放高亮。</p>
    {/if}
  {/if}
</div>
