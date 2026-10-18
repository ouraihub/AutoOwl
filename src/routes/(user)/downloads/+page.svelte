<script lang="ts">
  import type { AppError, DownloadRecord, DownloadState } from '$lib/domain/contracts';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { downloadService } from '$lib/services/user/download-service';
  import { toAppError } from '$lib/services/errors';
  import { notify } from '$lib/stores/toast';
  import { formatDateTime } from '$lib/format';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Tabs from '$lib/components/ui/tabs/index.js';
  import * as Empty from '$lib/components/ui/empty/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import DownloadQueue from '$lib/components/music/DownloadQueue.svelte';
  import { downloadsStore } from '$lib/stores/downloads.svelte';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';

  type Filter = 'all' | 'active' | 'completed' | 'failed' | 'records';

  const IN_FLIGHT: readonly DownloadState[] = ['queued', 'resolving', 'downloading', 'processing'];

  /** Matches the backend default for `list_download_records`. */
  const RECORDS_PAGE_SIZE = 20;

  let filter = $state<Filter>('all');
  let records = $state<DownloadRecord[]>([]);
  let recordsTotal = $state(0);
  let recordsPage = $state(1);
  let recordsLoading = $state(false);
  let recordsError = $state<AppError | null>(null);
  let confirmClear = $state(false);
  let clearing = $state(false);

  /**
   * The store is a process-wide singleton and `PlayerBar` owns the
   * `download:changed` / `download:progress` subscriptions for the whole app, so
   * this page only pulls the initial list — calling `attach()` here would double
   * every listener.
   */
  $effect(() => {
    downloadsStore.load();
  });

  /** Records load lazily: the tab is off the default path. */
  $effect(() => {
    if (filter !== 'records') return;
    void loadRecords(recordsPage);
  });

  async function loadRecords(page: number): Promise<void> {
    recordsLoading = true;
    try {
      const result = await downloadService.listRecords(page, RECORDS_PAGE_SIZE);
      records = result.records;
      recordsTotal = result.total;
      // The backend clamps an overshooting page, so follow it back.
      recordsPage = result.page;
      recordsError = null;
    } catch (cause) {
      recordsError = toAppError(cause);
    } finally {
      recordsLoading = false;
    }
  }

  async function clearRecords(): Promise<void> {
    clearing = true;
    try {
      const removed = await downloadService.clearRecords();
      notify.success('已清空下载记录', `共删除 ${removed} 条`);
      confirmClear = false;
      recordsPage = 1;
      await loadRecords(1);
    } catch (cause) {
      notify.fromError(toAppError(cause), '清空记录失败');
    } finally {
      clearing = false;
    }
  }

  const tasks = $derived(
    filter === 'all' || filter === 'records'
      ? downloadsStore.tasks
      : filter === 'active'
        ? downloadsStore.tasks.filter((task) => IN_FLIGHT.includes(task.state))
        : downloadsStore.tasks.filter((task) => task.state === filter)
  );

  const tabs: { value: Filter; label: string }[] = $derived([
    { value: 'all', label: `全部 ${downloadsStore.tasks.length}` },
    { value: 'active', label: `进行中 ${downloadsStore.activeCount}` },
    { value: 'completed', label: `已完成 ${downloadsStore.completedCount}` },
    { value: 'failed', label: `失败 ${downloadsStore.failedCount}` },
    { value: 'records', label: '历史记录' }
  ]);

  const emptyHint = $derived(
    filter === 'all'
      ? '在搜索或歌单页点击下载按钮，任务会出现在这里。'
      : '当前筛选条件下没有任务，换个标签看看。'
  );
</script>

<svelte:head><title>下载记录 - AutoOwl</title></svelte:head>

<div class="flex flex-col gap-4 p-6">
  <div class="flex flex-wrap items-center justify-between gap-3">
    <p class="text-sm text-muted-foreground">
      进行中 <span class="font-medium text-foreground tabular-nums">{downloadsStore.activeCount}</span>
      · 已完成
      <span class="font-medium text-foreground tabular-nums">{downloadsStore.completedCount}</span>
      · 失败
      <span class="font-medium text-foreground tabular-nums">{downloadsStore.failedCount}</span>
    </p>
    <Button variant="outline" onclick={() => downloadsStore.openFolder()}>
      <FolderOpen />
      打开下载目录
    </Button>
  </div>

  {#if downloadsStore.error}
    <Alert.Root variant="destructive">
      <Alert.Title>下载列表加载失败</Alert.Title>
      <Alert.Description class="flex flex-wrap items-center gap-3">
        <span>{downloadsStore.error.message}（{downloadsStore.error.code}）</span>
        <Button
          variant="outline"
          size="sm"
          onclick={() => {
            downloadsStore.clearError();
            downloadsStore.load();
          }}
        >
          <RotateCcw />
          重试
        </Button>
      </Alert.Description>
    </Alert.Root>
  {/if}

  <Tabs.Root value={filter} onValueChange={(value) => (filter = value as Filter)}>
    <Tabs.List>
      {#each tabs as tab (tab.value)}
        <Tabs.Trigger value={tab.value}>{tab.label}</Tabs.Trigger>
      {/each}
    </Tabs.List>
  </Tabs.Root>

  {#if filter === 'records'}
    {#if recordsError}
      <Alert.Root variant="destructive">
        <Alert.Title>下载记录加载失败</Alert.Title>
        <Alert.Description class="flex flex-wrap items-center gap-3">
          <span>{recordsError.message}（{recordsError.code}）</span>
          <Button variant="outline" size="sm" onclick={() => loadRecords(recordsPage)}>
            <RotateCcw />
            重试
          </Button>
        </Alert.Description>
      </Alert.Root>
    {/if}

    <div class="flex flex-wrap items-center justify-between gap-3">
      <span class="text-sm text-muted-foreground tabular-nums">共 {recordsTotal} 条记录</span>
      <Button
        variant="outline"
        size="sm"
        disabled={recordsTotal === 0 || clearing}
        onclick={() => (confirmClear = true)}
      >
        <Trash2 />
        清空记录
      </Button>
    </div>

    {#if recordsLoading && records.length === 0}
      <div class="flex flex-col gap-2">
        {#each Array.from({ length: 6 }) as _, i (i)}
          <Skeleton class="h-12 w-full" />
        {/each}
      </div>
    {:else if records.length === 0}
      <Empty.Root class="border">
        <Empty.Header>
          <Empty.Media variant="icon">
            <DownloadIcon />
          </Empty.Media>
          <Empty.Title>暂无下载记录</Empty.Title>
          <Empty.Description>下载完成的歌曲会留在这里，清空后不影响已下载的文件。</Empty.Description>
        </Empty.Header>
      </Empty.Root>
    {:else}
      <div class="rounded-lg border border-border">
        <Table.Root>
          <Table.Header>
            <Table.Row>
              <Table.Head>歌曲</Table.Head>
              <Table.Head>歌手</Table.Head>
              <Table.Head>保存路径</Table.Head>
              <Table.Head class="w-24">状态</Table.Head>
              <Table.Head class="w-40">时间</Table.Head>
            </Table.Row>
          </Table.Header>
          <Table.Body>
            {#each records as record (record.id)}
              <Table.Row>
                <Table.Cell class="max-w-0">
                  <span class="block truncate font-medium">{record.title}</span>
                </Table.Cell>
                <Table.Cell class="max-w-0 text-muted-foreground">
                  <span class="block truncate">{record.artist || '未知艺术家'}</span>
                </Table.Cell>
                <Table.Cell class="max-w-0 text-muted-foreground">
                  <span class="block truncate text-sm" title={record.targetPath}>
                    {record.targetPath}
                  </span>
                </Table.Cell>
                <Table.Cell>
                  <Badge variant={record.state === 'completed' ? 'secondary' : 'outline'}>
                    {record.state}
                  </Badge>
                </Table.Cell>
                <Table.Cell class="text-sm text-muted-foreground tabular-nums">
                  {formatDateTime(record.createdAt)}
                </Table.Cell>
              </Table.Row>
            {/each}
          </Table.Body>
        </Table.Root>
      </div>

      {#if recordsTotal > RECORDS_PAGE_SIZE}
        <div class="flex items-center justify-between gap-3">
          <Button
            variant="outline"
            size="sm"
            disabled={recordsPage <= 1 || recordsLoading}
            onclick={() => (recordsPage -= 1)}
          >
            上一页
          </Button>
          <span class="text-sm text-muted-foreground tabular-nums">
            第 {recordsPage} / {Math.max(1, Math.ceil(recordsTotal / RECORDS_PAGE_SIZE))} 页
          </span>
          <Button
            variant="outline"
            size="sm"
            disabled={recordsPage * RECORDS_PAGE_SIZE >= recordsTotal || recordsLoading}
            onclick={() => (recordsPage += 1)}
          >
            下一页
          </Button>
        </div>
      {/if}
    {/if}
  {:else if downloadsStore.loading && !downloadsStore.hasTasks}
    <div class="flex flex-col gap-3">
      {#each Array.from({ length: 4 }) as _, i (i)}
        <Skeleton class="h-28 w-full rounded-lg" />
      {/each}
    </div>
  {:else if tasks.length === 0}
    <Empty.Root class="border">
      <Empty.Header>
        <Empty.Media variant="icon">
          <DownloadIcon />
        </Empty.Media>
        <Empty.Title>暂无下载任务</Empty.Title>
        <Empty.Description>{emptyHint}</Empty.Description>
      </Empty.Header>
    </Empty.Root>
  {:else}
    <DownloadQueue {tasks} />
  {/if}
</div>

<Dialog.Root open={confirmClear} onOpenChange={(open) => (confirmClear = open)}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>清空下载记录</Dialog.Title>
      <Dialog.Description>
        将删除全部 {recordsTotal} 条下载记录，无法撤销。磁盘上已下载的文件不会被删除，去重索引也会保留，已下载过的歌曲仍不会重复下载。
      </Dialog.Description>
    </Dialog.Header>
    <Dialog.Footer>
      <Button variant="outline" disabled={clearing} onclick={() => (confirmClear = false)}>
        取消
      </Button>
      <Button variant="destructive" disabled={clearing} onclick={clearRecords}>
        {clearing ? '清空中…' : '确认清空'}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
