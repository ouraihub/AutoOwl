<script lang="ts">
  import type { MediaToolStatus } from '$lib/domain/contracts';
  import { mediaToolService } from '$lib/services/admin/media-tool-service';
  import { notify } from '$lib/stores/toast';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import { Separator } from '$lib/components/ui/separator/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import CircleX from '@lucide/svelte/icons/circle-x';

  /**
   * `getStatus` resolves even when a tool is missing — absence arrives as
   * `ffmpeg: false` plus an error string, so there is no rejection path here.
   */
  let status = $state<MediaToolStatus | null>(null);
  let loading = $state(false);

  async function load(): Promise<void> {
    loading = true;
    status = await mediaToolService.getStatus();
    loading = false;
  }

  $effect(() => {
    if (status === null && !loading) void load();
  });

  async function refresh(): Promise<void> {
    await load();
    const current = status;
    if (current && current.ffmpeg && current.ffprobe) {
      notify.success('媒体工具就绪', 'ffmpeg 与 ffprobe 均已找到');
    } else {
      notify.info('已重新检测', '仍有工具缺失');
    }
  }

  const tools = $derived(
    status === null
      ? []
      : [
          {
            name: 'ffmpeg',
            available: status.ffmpeg,
            path: status.ffmpegPath,
            error: status.ffmpegError,
            purpose: '写入元数据、转码与视频导出'
          },
          {
            name: 'ffprobe',
            available: status.ffprobe,
            path: status.ffprobePath,
            error: status.ffprobeError,
            purpose: '读取音频时长与格式信息'
          }
        ]
  );

  const missing = $derived(tools.filter((tool) => !tool.available).map((tool) => tool.name));
</script>

<div class="mx-auto max-w-3xl space-y-6 p-6">
  <header class="flex items-start justify-between gap-4">
    <div>
      <h1 class="text-2xl font-semibold tracking-tight">媒体工具</h1>
      <p class="text-muted-foreground mt-1 text-sm">
        ffmpeg 与 ffprobe 的检测结果。缺失时下载仍可用，但元数据写入与视频导出会被跳过。
      </p>
    </div>
    <Button variant="outline" size="sm" disabled={loading} onclick={refresh} class="shrink-0">
      <RefreshCw class="size-4" />
      {loading ? '检测中…' : '重新检测'}
    </Button>
  </header>

  {#if status === null}
    <Card.Root>
      <Card.Content class="space-y-4 py-6">
        <Skeleton class="h-5 w-28" />
        <Skeleton class="h-4 w-full" />
        <Skeleton class="h-5 w-28" />
        <Skeleton class="h-4 w-2/3" />
      </Card.Content>
    </Card.Root>
  {:else}
    {#if missing.length > 0}
      <Alert.Root variant="destructive">
        <TriangleAlert />
        <Alert.Title>缺少 {missing.join(' 与 ')}</Alert.Title>
        <Alert.Description>
          <p>音乐下载不受影响，但以下功能不可用：写入封面与标签、格式转换、生成导出视频。</p>
          <p class="mt-2">
            安装方式：Windows 可用 <code class="text-foreground">winget install Gyan.FFmpeg</code>
            或从 ffmpeg.org 下载压缩包，解压后把 <code class="text-foreground">bin</code> 目录加入系统
            <code class="text-foreground">PATH</code>；macOS 用
            <code class="text-foreground">brew install ffmpeg</code>；Linux 用发行版包管理器安装
            <code class="text-foreground">ffmpeg</code>。安装完成后点击右上角「重新检测」。
          </p>
        </Alert.Description>
      </Alert.Root>
    {/if}

    <Card.Root>
      <Card.Header>
        <Card.Title>检测结果</Card.Title>
        <Card.Description>可执行文件按系统 PATH 与内置路径依次查找。</Card.Description>
      </Card.Header>
      <Card.Content class="space-y-4">
        {#each tools as tool, index (tool.name)}
          {#if index > 0}
            <Separator />
          {/if}
          <div class="space-y-2">
            <div class="flex flex-wrap items-center gap-2">
              <code class="text-sm font-medium">{tool.name}</code>
              {#if tool.available}
                <Badge class="gap-1">
                  <CircleCheck class="size-3" />
                  可用
                </Badge>
              {:else}
                <Badge variant="destructive" class="gap-1">
                  <CircleX class="size-3" />
                  缺失
                </Badge>
              {/if}
              <span class="text-muted-foreground text-xs">{tool.purpose}</span>
            </div>
            {#if tool.available}
              <p class="text-muted-foreground text-xs break-all">
                路径：<code class="text-foreground">{tool.path ?? '（未返回路径）'}</code>
              </p>
            {:else}
              <p class="text-destructive text-xs break-all">
                {tool.error ?? '未找到可执行文件。'}
              </p>
            {/if}
          </div>
        {/each}
      </Card.Content>
    </Card.Root>
  {/if}
</div>
