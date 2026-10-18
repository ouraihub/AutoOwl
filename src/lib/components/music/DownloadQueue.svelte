<script lang="ts" module>
  import type { DownloadState } from '$lib/domain/contracts';
  import type { BadgeVariant } from '$lib/components/ui/badge/index.js';

  /** Chinese label for every `DownloadState`. */
  const STATE_LABELS: Record<DownloadState, string> = {
    queued: '排队中',
    resolving: '解析中',
    downloading: '下载中',
    processing: '处理中',
    completed: '已完成',
    failed: '失败',
    cancelRequested: '取消中',
    cancelled: '已取消'
  };

  /** `completed` reads as neutral-positive, `failed` as destructive, everything live as default. */
  const STATE_VARIANTS: Record<DownloadState, BadgeVariant> = {
    queued: 'default',
    resolving: 'default',
    downloading: 'default',
    processing: 'default',
    completed: 'secondary',
    failed: 'destructive',
    cancelRequested: 'outline',
    cancelled: 'outline'
  };

  /** Tasks that are still moving: cancellable, and their progress bar is meaningful. */
  const IN_FLIGHT: readonly DownloadState[] = ['queued', 'resolving', 'downloading', 'processing'];
</script>

<script lang="ts">
  import type { DownloadTask } from '$lib/domain/contracts';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Progress } from '$lib/components/ui/progress/index.js';
  import * as Tooltip from '$lib/components/ui/tooltip/index.js';
  import SourceBadge from './SourceBadge.svelte';
  import { clampRatio, formatBytes, formatSpeed } from '$lib/format';
  import { downloadsStore } from '$lib/stores/downloads.svelte';
  import X from '@lucide/svelte/icons/x';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import CircleAlert from '@lucide/svelte/icons/circle-alert';

  let { tasks }: { tasks: DownloadTask[] } = $props();
</script>

<Tooltip.Provider>
  <ul class="flex flex-col gap-3">
    {#each tasks as task (task.id)}
      {@const inFlight = IN_FLIGHT.includes(task.state)}
      {@const speed = formatSpeed(task.speedBytesPerSecond)}
      {@const artists = task.song.artists.map((artist) => artist.name).join(' / ')}
      <li class="rounded-lg border border-border bg-card p-4">
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0 flex-1">
            <div class="flex min-w-0 items-center gap-2">
              <span class="truncate font-medium" title={task.song.title}>{task.song.title}</span>
              <SourceBadge source={task.song.ref.source} size="sm" />
              <Badge variant={STATE_VARIANTS[task.state]}>{STATE_LABELS[task.state]}</Badge>
              {#if task.skipped}
                <Badge variant="outline">已存在</Badge>
              {/if}
            </div>
            <p class="mt-1 truncate text-sm text-muted-foreground" title={artists}>
              {artists || '未知艺术家'}
            </p>
          </div>

          <div class="flex shrink-0 items-center gap-1">
            {#if inFlight}
              <Tooltip.Root>
                <Tooltip.Trigger>
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      variant="ghost"
                      size="icon"
                      aria-label={`取消下载 ${task.song.title}`}
                      onclick={() => downloadsStore.cancel(task.id)}
                    >
                      <X />
                    </Button>
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Content>取消</Tooltip.Content>
              </Tooltip.Root>
            {/if}
            {#if task.state === 'failed'}
              <Tooltip.Root>
                <Tooltip.Trigger>
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      variant="ghost"
                      size="icon"
                      aria-label={`重试下载 ${task.song.title}`}
                      onclick={() => downloadsStore.retry(task.id)}
                    >
                      <RotateCcw />
                    </Button>
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Content>重试</Tooltip.Content>
              </Tooltip.Root>
            {/if}
            {#if task.state === 'completed'}
              <Tooltip.Root>
                <Tooltip.Trigger>
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      variant="ghost"
                      size="icon"
                      aria-label={`打开 ${task.song.title} 所在文件夹`}
                      onclick={() => downloadsStore.openFolder(task.id)}
                    >
                      <FolderOpen />
                    </Button>
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Content>打开所在文件夹</Tooltip.Content>
              </Tooltip.Root>
            {/if}
          </div>
        </div>

        <Progress class="mt-3" max={1} value={clampRatio(task.progress)} />

        <div class="mt-2 flex items-center justify-between gap-4 text-xs text-muted-foreground">
          <span class="tabular-nums">
            {formatBytes(task.downloadedBytes, '0 B')} / {formatBytes(task.totalBytes, '未知大小')}
            {#if speed}<span class="ml-2">{speed}</span>{/if}
          </span>
          <span class="tabular-nums">{Math.round(clampRatio(task.progress) * 100)}%</span>
        </div>

        <Tooltip.Root>
          <Tooltip.Trigger class="mt-1 block w-full text-left">
            <span class="block truncate text-xs text-muted-foreground">{task.targetPath}</span>
          </Tooltip.Trigger>
          <Tooltip.Content class="max-w-md break-all">{task.targetPath}</Tooltip.Content>
        </Tooltip.Root>

        {#if task.warning}
          <p class="mt-2 flex items-start gap-1.5 text-xs text-muted-foreground">
            <TriangleAlert class="mt-0.5 size-3.5 shrink-0" />
            <span>{task.warning}</span>
          </p>
        {/if}

        {#if task.error}
          <div class="mt-2 flex items-start gap-1.5 text-xs text-destructive">
            <CircleAlert class="mt-0.5 size-3.5 shrink-0" />
            <p class="min-w-0 flex-1">
              <span class="font-medium">{task.error.code}</span>
              <span class="ml-1">{task.error.message}</span>
            </p>
            {#if task.error.retryable}
              <Button
                variant="ghost"
                size="sm"
                class="h-6 shrink-0 px-2 text-xs"
                onclick={() => downloadsStore.retry(task.id)}
              >
                重试
              </Button>
            {/if}
          </div>
        {/if}
      </li>
    {/each}
  </ul>
</Tooltip.Provider>
