<script lang="ts">
  import type { AppError, LocalTrack, Song } from '$lib/domain/contracts';
  import * as Tabs from '$lib/components/ui/tabs/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import * as Empty from '$lib/components/ui/empty/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Pagination from '$lib/components/ui/pagination/index.js';
  import * as Tooltip from '$lib/components/ui/tooltip/index.js';
  import { localMusicService } from '$lib/services/user/local-music-service';
  import { playerStore } from '$lib/stores/player.svelte';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { toAppError } from '$lib/services/errors';
  import { notify } from '$lib/stores/toast';
  import { formatBytes, formatDateTime, formatDuration } from '$lib/format';
  import Play from '@lucide/svelte/icons/play';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import Search from '@lucide/svelte/icons/search';
  import ChevronLeft from '@lucide/svelte/icons/chevron-left';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import ListX from '@lucide/svelte/icons/list-x';
  import Copy from '@lucide/svelte/icons/copy';

  /** Duplicate groups are tall, so this view pages in tens. */
  const DUPLICATES_PAGE_SIZE = 10;

  let tracks = $state<LocalTrack[]>([]);
  let page = $state(1);
  let total = $state(0);
  let loading = $state(false);
  let error = $state<AppError | null>(null);
  let refreshing = $state(false);
  /** Raw input value; `filter` trails it by the debounce interval. */
  let filterInput = $state('');
  let filter = $state('');
  let pending = $state<LocalTrack | null>(null);
  let view = $state<'all' | 'duplicates'>('all');
  let duplicateGroups = $state<LocalTrack[][]>([]);
  let duplicateTotal = $state(0);
  let duplicatePage = $state(1);
  let duplicatesLoading = $state(false);
  let duplicatesError = $state<AppError | null>(null);
  let removing = $state(false);

  const pageSize = $derived(settingsStore.settings?.pageSize ?? 20);

  $effect(() => {
    if (!settingsStore.loaded && !settingsStore.loading) settingsStore.load();
  });

  /** Debounce the filter so typing does not fire one IPC round trip per keystroke. */
  $effect(() => {
    const next = filterInput.trim();
    if (next === filter) return;

    const timer = setTimeout(() => {
      filter = next;
      page = 1;
    }, 300);
    return () => clearTimeout(timer);
  });

  $effect(() => {
    void load(page, pageSize, filter);
  });

  /** Duplicate groups load lazily: the tab is off the default path. */
  $effect(() => {
    if (view !== 'duplicates') return;
    void loadDuplicates(duplicatePage);
  });

  async function loadDuplicates(nextPage: number): Promise<void> {
    duplicatesLoading = true;
    try {
      const result = await localMusicService.duplicates(nextPage, DUPLICATES_PAGE_SIZE);
      duplicateGroups = result.groups;
      duplicateTotal = result.total;
      duplicatePage = result.page;
      duplicatesError = null;
    } catch (cause) {
      duplicatesError = toAppError(cause);
    } finally {
      duplicatesLoading = false;
    }
  }

  async function load(nextPage: number, size: number, query: string): Promise<void> {
    loading = true;
    try {
      const result = await localMusicService.list(nextPage, size, query || undefined);
      tracks = result.items;
      total = result.total;
      error = null;
    } catch (cause) {
      error = toAppError(cause);
    } finally {
      loading = false;
    }
  }

  async function rescan(): Promise<void> {
    refreshing = true;
    try {
      const report = await localMusicService.refresh();
      notify.success(
        '扫描完成',
        `共 ${report.seen} 个文件 · 新增 ${report.added} · 更新 ${report.updated} · 移除 ${report.removed}`
      );
      page = 1;
      await load(1, pageSize, filter);
    } catch (cause) {
      notify.fromError(toAppError(cause), '扫描失败');
    } finally {
      refreshing = false;
    }
  }

  /** A local row carries the same identity as its remote song, so playback reuses it. */
  async function play(track: LocalTrack): Promise<void> {
    const song: Song = {
      ref: track.ref,
      title: track.title,
      artists: track.artists,
      album: track.album,
      durationMs: track.durationMs,
      coverUrl: track.coverUrl
    };
    await playerStore.playSong(song);
  }

  async function remove(deleteFile: boolean): Promise<void> {
    const track = pending;
    if (!track) return;

    removing = true;
    try {
      await localMusicService.remove(track.id, deleteFile);
      notify.success(deleteFile ? '已删除文件' : '已移出列表', track.title);
      pending = null;
      await load(page, pageSize, filter);
    } catch (cause) {
      notify.fromError(toAppError(cause), '删除失败');
    } finally {
      removing = false;
    }
  }
</script>

<svelte:head><title>本地音乐 - AutoOwl</title></svelte:head>

<div class="flex flex-col gap-4 p-6">
  <div class="flex flex-wrap items-center justify-between gap-3">
    <!-- The filter only applies to the full listing; duplicates are grouped whole. -->
    {#if view === 'all'}
      <div class="relative w-full max-w-sm">
        <Search
          class="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          bind:value={filterInput}
          class="pl-9"
          placeholder="按歌名或歌手筛选"
          aria-label="按歌名或歌手筛选本地音乐"
        />
      </div>
    {:else}
      <span class="text-sm text-muted-foreground">同名同歌手的文件会归为一组。</span>
    {/if}
    <div class="flex items-center gap-3">
      <span class="text-sm text-muted-foreground tabular-nums">共 {total} 首</span>
      <Button variant="outline" disabled={refreshing} onclick={rescan}>
        <RefreshCw class={refreshing ? 'animate-spin' : undefined} />
        {refreshing ? '扫描中…' : '重新扫描'}
      </Button>
    </div>
  </div>

  <Tabs.Root value={view} onValueChange={(value) => (view = value as 'all' | 'duplicates')}>
    <Tabs.List>
      <Tabs.Trigger value="all">全部曲目</Tabs.Trigger>
      <Tabs.Trigger value="duplicates">重复曲目</Tabs.Trigger>
    </Tabs.List>
  </Tabs.Root>

  {#if view === 'duplicates'}
    {#if duplicatesError}
      <Alert.Root variant="destructive">
        <Alert.Title>重复曲目加载失败</Alert.Title>
        <Alert.Description class="flex flex-wrap items-center gap-3">
          <span>{duplicatesError.message}（{duplicatesError.code}）</span>
          <Button variant="outline" size="sm" onclick={() => loadDuplicates(duplicatePage)}>
            <RotateCcw />
            重试
          </Button>
        </Alert.Description>
      </Alert.Root>
    {/if}

    {#if duplicatesLoading && duplicateGroups.length === 0}
      <div class="flex flex-col gap-2">
        {#each Array.from({ length: 4 }) as _, i (i)}
          <Skeleton class="h-24 w-full rounded-lg" />
        {/each}
      </div>
    {:else if duplicateGroups.length === 0}
      <Empty.Root class="border">
        <Empty.Header>
          <Empty.Media variant="icon">
            <Copy />
          </Empty.Media>
          <Empty.Title>没有重复曲目</Empty.Title>
          <Empty.Description>音乐库里没有同名同歌手的重复文件。</Empty.Description>
        </Empty.Header>
      </Empty.Root>
    {:else}
      <p class="text-sm text-muted-foreground tabular-nums">共 {duplicateTotal} 组重复</p>
      <div class="flex flex-col gap-3">
        {#each duplicateGroups as group, index (group[0]?.id ?? index)}
          <div class="rounded-lg border border-border">
            <div class="flex items-center justify-between gap-3 border-b border-border px-4 py-2">
              <span class="truncate font-medium">{group[0]?.title ?? '未知曲目'}</span>
              <Badge variant="secondary">{group.length} 个文件</Badge>
            </div>
            <Table.Root>
              <Table.Body>
                {#each group as track (track.id)}
                  <Table.Row class="group/dup">
                    <Table.Cell class="max-w-0">
                      <span class="block truncate text-sm" title={track.path}>{track.path}</span>
                    </Table.Cell>
                    <Table.Cell class="w-24 text-right text-sm text-muted-foreground tabular-nums">
                      {formatBytes(track.fileSize)}
                    </Table.Cell>
                    <Table.Cell class="w-20">
                      <Badge variant="outline">{track.format.toUpperCase()}</Badge>
                    </Table.Cell>
                    <Table.Cell class="w-40 text-sm text-muted-foreground tabular-nums">
                      {formatDateTime(track.modifiedAt)}
                    </Table.Cell>
                    <Table.Cell class="w-24">
                      <div class="flex items-center justify-end gap-1">
                        <Button
                          variant="ghost"
                          size="icon"
                          aria-label={`播放 ${track.title}`}
                          onclick={() => play(track)}
                        >
                          <Play />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          aria-label={`删除 ${track.title}`}
                          onclick={() => (pending = track)}
                        >
                          <Trash2 />
                        </Button>
                      </div>
                    </Table.Cell>
                  </Table.Row>
                {/each}
              </Table.Body>
            </Table.Root>
          </div>
        {/each}
      </div>

      {#if duplicateTotal > DUPLICATES_PAGE_SIZE}
        <div class="flex items-center justify-between gap-3">
          <Button
            variant="outline"
            size="sm"
            disabled={duplicatePage <= 1 || duplicatesLoading}
            onclick={() => (duplicatePage -= 1)}
          >
            <ChevronLeft class="size-4" />
            上一页
          </Button>
          <span class="text-sm text-muted-foreground tabular-nums">
            第 {duplicatePage} / {Math.max(1, Math.ceil(duplicateTotal / DUPLICATES_PAGE_SIZE))} 页
          </span>
          <Button
            variant="outline"
            size="sm"
            disabled={duplicatePage * DUPLICATES_PAGE_SIZE >= duplicateTotal || duplicatesLoading}
            onclick={() => (duplicatePage += 1)}
          >
            下一页
            <ChevronRight class="size-4" />
          </Button>
        </div>
      {/if}
    {/if}
  {:else}

  {#if error}
    <Alert.Root variant="destructive">
      <Alert.Title>本地音乐加载失败</Alert.Title>
      <Alert.Description class="flex flex-wrap items-center gap-3">
        <span>{error.message}（{error.code}）</span>
        <Button variant="outline" size="sm" onclick={() => load(page, pageSize, filter)}>
          <RotateCcw />
          重试
        </Button>
      </Alert.Description>
    </Alert.Root>
  {/if}

  {#if loading && tracks.length === 0}
    <div class="flex flex-col gap-2">
      {#each Array.from({ length: 8 }) as _, i (i)}
        <Skeleton class="h-12 w-full" />
      {/each}
    </div>
  {:else if tracks.length === 0}
    <Empty.Root class="border">
      <Empty.Header>
        <Empty.Media variant="icon">
          <HardDrive />
        </Empty.Media>
        <Empty.Title>{filter ? '没有匹配的歌曲' : '本地音乐库是空的'}</Empty.Title>
        <Empty.Description>
          {filter
            ? '换个关键词再试试，或清空筛选条件查看全部歌曲。'
            : '下载歌曲后会自动入库；如果你手动放入过音频文件，点击下方按钮重新扫描下载目录。'}
        </Empty.Description>
      </Empty.Header>
      <Empty.Content>
        <Button variant="outline" disabled={refreshing} onclick={rescan}>
          <RefreshCw class={refreshing ? 'animate-spin' : undefined} />
          {refreshing ? '扫描中…' : '重新扫描'}
        </Button>
      </Empty.Content>
    </Empty.Root>
  {:else}
    <Tooltip.Provider>
      <div class="rounded-lg border border-border">
        <Table.Root>
          <Table.Header>
            <Table.Row>
              <Table.Head>歌曲</Table.Head>
              <Table.Head>歌手</Table.Head>
              <Table.Head>专辑</Table.Head>
              <Table.Head class="w-20 text-right">时长</Table.Head>
              <Table.Head class="w-24 text-right">大小</Table.Head>
              <Table.Head class="w-20">格式</Table.Head>
              <Table.Head class="w-40">修改时间</Table.Head>
              <Table.Head class="w-24 text-right">操作</Table.Head>
            </Table.Row>
          </Table.Header>
          <Table.Body>
            {#each tracks as track (track.id)}
              {@const artists = track.artists.map((artist) => artist.name).join(' / ')}
              <Table.Row class="group/row">
                <Table.Cell class="max-w-0">
                  <Tooltip.Root>
                    <Tooltip.Trigger class="block w-full text-left">
                      <span class="block truncate font-medium">{track.title}</span>
                    </Tooltip.Trigger>
                    <Tooltip.Content class="max-w-md break-all">{track.path}</Tooltip.Content>
                  </Tooltip.Root>
                </Table.Cell>
                <Table.Cell class="max-w-0 text-muted-foreground">
                  <span class="block truncate" title={artists}>{artists || '未知艺术家'}</span>
                </Table.Cell>
                <Table.Cell class="max-w-0 text-muted-foreground">
                  <span class="block truncate" title={track.album?.name ?? ''}>
                    {track.album?.name ?? '-'}
                  </span>
                </Table.Cell>
                <Table.Cell class="text-right text-sm text-muted-foreground tabular-nums">
                  {formatDuration(track.durationMs)}
                </Table.Cell>
                <Table.Cell class="text-right text-sm text-muted-foreground tabular-nums">
                  {formatBytes(track.fileSize)}
                </Table.Cell>
                <Table.Cell>
                  <Badge variant="outline">{track.format.toUpperCase()}</Badge>
                </Table.Cell>
                <Table.Cell class="text-sm text-muted-foreground tabular-nums">
                  {formatDateTime(track.modifiedAt)}
                </Table.Cell>
                <Table.Cell>
                  <div
                    class="flex items-center justify-end gap-1 opacity-0 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100"
                  >
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={`播放 ${track.title}`}
                      onclick={() => play(track)}
                    >
                      <Play />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={`删除 ${track.title}`}
                      onclick={() => (pending = track)}
                    >
                      <Trash2 />
                    </Button>
                  </div>
                </Table.Cell>
              </Table.Row>
            {/each}
          </Table.Body>
        </Table.Root>
      </div>
    </Tooltip.Provider>
  {/if}

  {#if total > pageSize}
    <Pagination.Root count={total} perPage={pageSize} bind:page>
      {#snippet children({ pages, currentPage })}
        <Pagination.Content>
          <Pagination.Item>
            <Pagination.PrevButton>
              <ChevronLeft class="size-4" />
              <span>上一页</span>
            </Pagination.PrevButton>
          </Pagination.Item>
          {#each pages as pageItem (pageItem.key)}
            {#if pageItem.type === 'ellipsis'}
              <Pagination.Item>
                <Pagination.Ellipsis />
              </Pagination.Item>
            {:else}
              <Pagination.Item>
                <Pagination.Link page={pageItem} isActive={currentPage === pageItem.value}>
                  {pageItem.value}
                </Pagination.Link>
              </Pagination.Item>
            {/if}
          {/each}
          <Pagination.Item>
            <Pagination.NextButton>
              <span>下一页</span>
              <ChevronRight class="size-4" />
            </Pagination.NextButton>
          </Pagination.Item>
        </Pagination.Content>
      {/snippet}
    </Pagination.Root>
  {/if}
  {/if}
</div>

<Dialog.Root open={pending !== null} onOpenChange={(open) => { if (!open) pending = null; }}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>移除《{pending?.title ?? ''}》</Dialog.Title>
      <Dialog.Description>
        请选择处理方式。「仅移出列表」只删除本地音乐库中的记录，磁盘上的文件保持不动；「同时删除文件」会把音频文件从磁盘上永久删除，无法撤销。
      </Dialog.Description>
    </Dialog.Header>

    {#if pending}
      <p class="rounded-md bg-muted px-3 py-2 text-xs break-all text-muted-foreground">
        {pending.path}
      </p>
    {/if}

    <Dialog.Footer class="gap-2 sm:justify-between">
      <Button variant="ghost" disabled={removing} onclick={() => (pending = null)}>取消</Button>
      <div class="flex flex-col gap-2 sm:flex-row">
        <Button variant="outline" disabled={removing} onclick={() => remove(false)}>
          <ListX />
          仅移出列表
        </Button>
        <Button variant="destructive" disabled={removing} onclick={() => remove(true)}>
          <Trash2 />
          同时删除文件
        </Button>
      </div>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
