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
  import Download from '@lucide/svelte/icons/download';
  import { toAppError } from '$lib/services/errors';

  /**
   * `getStatus` resolves even when a tool is missing — absence arrives as
   * `ffmpeg: false` plus an error string, so there is no rejection path here.
   */
  let status = $state<MediaToolStatus | null>(null);
  let loading = $state(false);
  let installing = $state(false);
  /** Download progress 0-100 while the portable install runs. */
  let percent = $state<number | null>(null);

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
    if (status?.ffmpeg) notify.success('媒体工具就绪', '已找到 ffmpeg');
    else notify.info('已重新检测', '仍未找到 ffmpeg');
  }

  async function install(): Promise<void> {
    installing = true;
    const portable = !status?.scoopAvailable;
    const unlisten = portable
      ? await mediaToolService.onDownloadProgress((received, total) => {
          percent = Math.floor((received / total) * 100);
        })
      : null;
    try {
      status = portable ? await mediaToolService.downloadPortable() : await mediaToolService.installWithScoop();
      notify.success('ffmpeg 已安装', '视频导出现在可用');
    } catch (cause) {
      notify.fromError(toAppError(cause), 'ffmpeg 安装失败');
    } finally {
      unlisten?.();
      percent = null;
      installing = false;
    }
  }

  // Only video export runs ffmpeg; tags, covers and lyrics are written without it.
  const tools = $derived(
    status === null
      ? []
      : [
          {
            name: 'ffmpeg',
            available: status.ffmpeg,
            path: status.ffmpegPath,
            error: status.ffmpegError,
            purpose: '视频导出'
          },
          {
            name: 'ffprobe',
            available: status.ffprobe,
            path: status.ffprobePath,
            error: status.ffprobeError,
            purpose: '随 ffmpeg 一起安装，目前没有功能依赖它'
          }
        ]
  );

  const ffmpegMissing = $derived(status !== null && !status.ffmpeg);
</script>

<div class="mx-auto max-w-3xl space-y-6 p-6">
  <header class="flex items-start justify-between gap-4">
    <div>
      <h1 class="text-2xl font-semibold tracking-tight">媒体工具</h1>
      <p class="text-muted-foreground mt-1 text-sm">
        只有「视频导出」需要 ffmpeg。搜索、播放、下载，以及写入封面、标签和歌词都不需要。
      </p>
    </div>
    <Button variant="outline" size="sm" disabled={loading || installing} onclick={refresh} class="shrink-0">
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
    {#if ffmpegMissing}
      <Alert.Root>
        <TriangleAlert />
        <Alert.Title>未安装 ffmpeg：视频导出不可用</Alert.Title>
        <Alert.Description>
          {#if status.scoopAvailable}
            <p>其他功能不受影响。用 Scoop 安装只需一步，不需要管理员权限。</p>
          {:else}
            <p>
              其他功能不受影响。可直接下载固定版本的 ffmpeg（约 109 MB，放到数据目录的 tools\ffmpeg，校验 SHA256）；
              在「设置 → 更新」开启 GitHub 加速代理后会走代理。
            </p>
          {/if}
          <div class="mt-3 flex flex-wrap items-center gap-2">
            <Button size="sm" disabled={installing} onclick={install}>
              <Download class="size-4" />
              {#if status.scoopAvailable}
                {installing ? '正在用 Scoop 安装…（可能需要几分钟）' : '用 Scoop 安装 ffmpeg'}
              {:else}
                {installing ? `正在下载… ${percent ?? 0}%` : '下载 ffmpeg'}
              {/if}
            </Button>
          </div>
          <p class="mt-3">
            也可手动安装：<code class="text-foreground">winget install Gyan.FFmpeg</code>，
            或从 ffmpeg.org 下载后把 <code class="text-foreground">bin</code> 目录加入
            <code class="text-foreground">PATH</code>，再点右上角「重新检测」。
          </p>
        </Alert.Description>
      </Alert.Root>
    {/if}

    <Card.Root>
      <Card.Header>
        <Card.Title>检测结果</Card.Title>
        <Card.Description>依次查找：环境变量 AUTOOWL_FFMPEG、程序目录、数据目录 tools\ffmpeg、系统 PATH、Scoop。</Card.Description>
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
