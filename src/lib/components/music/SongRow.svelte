<script lang="ts">
  import type { Song } from '$lib/domain/contracts';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import * as Tooltip from '$lib/components/ui/tooltip/index.js';
  import SourceBadge from './SourceBadge.svelte';
  import { formatDuration } from '$lib/format';
  import { cn } from '$lib/utils.js';
  import Play from '@lucide/svelte/icons/play';
  import Download from '@lucide/svelte/icons/download';
  import ListPlus from '@lucide/svelte/icons/list-plus';

  let {
    song,
    index,
    active = false,
    onplay,
    ondownload,
    onaddto,
    selectable
  }: {
    song: Song;
    index?: number;
    active?: boolean;
    onplay?: (song: Song) => void;
    ondownload?: (song: Song) => void;
    onaddto?: (song: Song) => void;
    /** Optional leading cell (e.g. a selection checkbox) rendered before the index. */
    selectable?: import('svelte').Snippet;
  } = $props();

  const artists = $derived(song.artists.map((artist) => artist.name).join(' / '));
  const duration = $derived(song.durationMs == null ? '' : formatDuration(song.durationMs));
</script>

<Table.Row class={cn('group/row', active && 'bg-accent')}>
  {#if selectable}
    <Table.Cell class="w-10 align-middle">{@render selectable()}</Table.Cell>
  {/if}
  <Table.Cell class="w-12 text-right text-sm tabular-nums text-muted-foreground">
    {index == null ? '' : index + 1}
  </Table.Cell>
  <Table.Cell class="max-w-0">
    <div class="flex min-w-0 items-center gap-2">
      <span class="truncate font-semibold" title={song.title}>{song.title}</span>
      <SourceBadge source={song.ref.source} size="sm" />
    </div>
  </Table.Cell>
  <Table.Cell class="max-w-0 text-muted-foreground">
    <span class="block truncate" title={artists}>{artists}</span>
  </Table.Cell>
  <Table.Cell class="max-w-0 text-muted-foreground">
    <span class="block truncate" title={song.album?.name ?? ''}>{song.album?.name ?? ''}</span>
  </Table.Cell>
  <Table.Cell class="w-16 text-right text-sm tabular-nums text-muted-foreground">
    {duration}
  </Table.Cell>
  <Table.Cell class="w-32">
    <Tooltip.Provider>
      <div
        class="flex items-center justify-end gap-1 opacity-0 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100"
      >
        {#if onplay}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="ghost"
                  size="icon"
                  aria-label={`播放 ${song.title}`}
                  onclick={() => onplay?.(song)}
                >
                  <Play />
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>播放</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if ondownload}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="ghost"
                  size="icon"
                  aria-label={`下载 ${song.title}`}
                  onclick={() => ondownload?.(song)}
                >
                  <Download />
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>下载</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onaddto}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="ghost"
                  size="icon"
                  aria-label={`将 ${song.title} 添加到歌单`}
                  onclick={() => onaddto?.(song)}
                >
                  <ListPlus />
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>添加到歌单</Tooltip.Content>
          </Tooltip.Root>
        {/if}
      </div>
    </Tooltip.Provider>
  </Table.Cell>
</Table.Row>
