<script lang="ts">
  import type { AppError, PlaybackEntry, Song } from '$lib/domain/contracts';
  import { toAppError } from '$lib/services/errors';
  import { historyService } from '$lib/services/user/history-service';
  import { downloadsStore } from '$lib/stores/downloads.svelte';
  import { playerStore } from '$lib/stores/player.svelte';
  import { notify } from '$lib/stores/toast';
  import { formatDateTime } from '$lib/format';
  import SourceBadge from '$lib/components/music/SourceBadge.svelte';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import * as Empty from '$lib/components/ui/empty/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import * as Tooltip from '$lib/components/ui/tooltip/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import HistoryIcon from '@lucide/svelte/icons/history';
  import PlayIcon from '@lucide/svelte/icons/play';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';

  const PAGE_SIZE = 50;
  const SKELETON_ROWS = Array.from({ length: 8 }, (_, index) => index);

  let entries = $state<PlaybackEntry[]>([]);
  let total = $state(0);
  let page = $state(1);
  let loading = $state(true);
  let error = $state<AppError | null>(null);
  let clearOpen = $state(false);
  let clearing = $state(false);

  const totalPages = $derived(Math.max(1, Math.ceil(total / PAGE_SIZE)));

  async function load(target: number): Promise<void> {
    loading = true;
    error = null;
    try {
      const result = await historyService.list(target, PAGE_SIZE);
      entries = result.items;
      total = result.total;
      page = result.page;
    } catch (cause) {
      error = toAppError(cause);
    } finally {
      loading = false;
    }
  }

  async function play(song: Song): Promise<void> {
    // Queue the visible page so next/previous walk the history listing.
    const queue = entries.map((entry) => entry.song);
    const index = queue.findIndex(
      (item) => item.ref.source === song.ref.source && item.ref.siteId === song.ref.siteId
    );
    await playerStore.setQueue(queue, index < 0 ? 0 : index);
    await playerStore.playSong(song);
  }

  async function download(song: Song): Promise<void> {
    try {
      await downloadsStore.enqueue(song);
      notify.success(`已加入下载队列：${song.title}`);
    } catch (cause) {
      notify.error(toAppError(cause).message);
    }
  }

  async function confirmClear(): Promise<void> {
    clearing = true;
    try {
      const removed = await historyService.clear();
      entries = [];
      total = 0;
      page = 1;
      clearOpen = false;
      notify.success(`已清空播放历史，共 ${removed} 条`);
    } catch (cause) {
      notify.error(toAppError(cause).message);
    } finally {
      clearing = false;
    }
  }

  function artistsOf(song: Song): string {
    return song.artists.map((artist) => artist.name).join(' / ');
  }

  $effect(() => {
    void load(1);
  });
</script>

<svelte:head><title>播放历史 - AutoOwl</title></svelte:head>

<div class="flex flex-col gap-4 p-6">
  <div class="flex items-center justify-between gap-4">
    <div class="flex flex-col gap-1">
      <h1 class="text-2xl font-semibold">播放历史</h1>
      <p class="text-sm text-muted-foreground">
        {total === 0 ? '暂无播放记录' : `共 ${total} 条记录，仅保留最近的播放`}
      </p>
    </div>
    <Button
      variant="outline"
      size="sm"
      disabled={total === 0 || loading}
      onclick={() => (clearOpen = true)}
    >
      <Trash2Icon class="size-4" />
      清空历史
    </Button>
  </div>

  {#if error !== null}
    <Alert.Root variant="destructive">
      <TriangleAlertIcon class="size-4" />
      <Alert.Title>播放历史加载失败</Alert.Title>
      <Alert.Description>{error.message}（错误码：{error.code}）</Alert.Description>
      <Alert.Action>
        <Button variant="outline" size="sm" onclick={() => void load(page)}>重试</Button>
      </Alert.Action>
    </Alert.Root>
  {:else if loading && entries.length === 0}
    <div class="flex flex-col gap-2">
      {#each SKELETON_ROWS as row (row)}
        <Skeleton class="h-11 w-full" />
      {/each}
    </div>
  {:else if entries.length === 0}
    <Empty.Root class="border">
      <Empty.Header>
        <Empty.Media variant="icon"><HistoryIcon /></Empty.Media>
        <Empty.Title>还没有播放记录</Empty.Title>
        <Empty.Description>播放任意歌曲后，这里会记录播放时间。</Empty.Description>
      </Empty.Header>
      <Empty.Content>
        <Button variant="outline" size="sm" href="/search">去搜索音乐</Button>
      </Empty.Content>
    </Empty.Root>
  {:else}
    <Tooltip.Provider>
      <Table.Root>
        <Table.Header>
          <Table.Row>
            <Table.Head class="w-12 text-right">#</Table.Head>
            <Table.Head>歌曲</Table.Head>
            <Table.Head>歌手</Table.Head>
            <Table.Head class="w-44">播放时间</Table.Head>
            <Table.Head class="w-24 text-right">操作</Table.Head>
          </Table.Row>
        </Table.Header>
        <Table.Body>
          {#each entries as entry, index (entry.id)}
            <Table.Row class="group/row">
              <Table.Cell class="w-12 text-right text-sm tabular-nums text-muted-foreground">
                {(page - 1) * PAGE_SIZE + index + 1}
              </Table.Cell>
              <Table.Cell class="max-w-0">
                <div class="flex min-w-0 items-center gap-2">
                  <span class="truncate font-medium" title={entry.song.title}>
                    {entry.song.title}
                  </span>
                  <SourceBadge source={entry.song.ref.source} size="sm" />
                </div>
              </Table.Cell>
              <Table.Cell class="max-w-0 text-muted-foreground">
                <span class="block truncate">{artistsOf(entry.song) || '未知歌手'}</span>
              </Table.Cell>
              <Table.Cell class="w-44 text-sm tabular-nums text-muted-foreground">
                {formatDateTime(entry.playedAt)}
              </Table.Cell>
              <Table.Cell class="w-24">
                <div
                  class="flex items-center justify-end gap-1 opacity-0 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100"
                >
                  <Tooltip.Root>
                    <Tooltip.Trigger>
                      {#snippet child({ props })}
                        <Button
                          {...props}
                          variant="ghost"
                          size="icon-sm"
                          aria-label={`播放 ${entry.song.title}`}
                          onclick={() => void play(entry.song)}
                        >
                          <PlayIcon class="size-4" />
                        </Button>
                      {/snippet}
                    </Tooltip.Trigger>
                    <Tooltip.Content>播放</Tooltip.Content>
                  </Tooltip.Root>

                  <Tooltip.Root>
                    <Tooltip.Trigger>
                      {#snippet child({ props })}
                        <Button
                          {...props}
                          variant="ghost"
                          size="icon-sm"
                          aria-label={`下载 ${entry.song.title}`}
                          onclick={() => void download(entry.song)}
                        >
                          <DownloadIcon class="size-4" />
                        </Button>
                      {/snippet}
                    </Tooltip.Trigger>
                    <Tooltip.Content>下载</Tooltip.Content>
                  </Tooltip.Root>
                </div>
              </Table.Cell>
            </Table.Row>
          {/each}
        </Table.Body>
      </Table.Root>
    </Tooltip.Provider>

    {#if totalPages > 1}
      <div class="flex items-center justify-end gap-2">
        <span class="text-sm text-muted-foreground">第 {page} / {totalPages} 页</span>
        <Button
          variant="outline"
          size="icon-sm"
          aria-label="上一页"
          disabled={page <= 1 || loading}
          onclick={() => void load(page - 1)}
        >
          <ChevronLeftIcon class="size-4" />
        </Button>
        <Button
          variant="outline"
          size="icon-sm"
          aria-label="下一页"
          disabled={page >= totalPages || loading}
          onclick={() => void load(page + 1)}
        >
          <ChevronRightIcon class="size-4" />
        </Button>
      </div>
    {/if}
  {/if}
</div>

<Dialog.Root bind:open={clearOpen}>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>清空播放历史</Dialog.Title>
      <Dialog.Description>
        将删除全部 {total} 条播放记录，此操作不可撤销。已下载的文件不受影响。
      </Dialog.Description>
    </Dialog.Header>
    <Dialog.Footer>
      <Button variant="outline" size="sm" onclick={() => (clearOpen = false)}>取消</Button>
      <Button variant="destructive" size="sm" disabled={clearing} onclick={() => void confirmClear()}>
        {clearing ? '清空中…' : '确认清空'}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
