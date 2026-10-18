<script lang="ts">
  import type { AppError, Diagnostics, UpdateInfo } from '$lib/domain/contracts';
  import { diagnosticsService } from '$lib/services/admin/diagnostics-service';
  import { toAppError } from '$lib/services/errors';
  import { notify } from '$lib/stores/toast';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import { Separator } from '$lib/components/ui/separator/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import Copy from '@lucide/svelte/icons/copy';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import ExternalLink from '@lucide/svelte/icons/external-link';

  let info = $state<Diagnostics | null>(null);
  let loadError = $state<AppError | null>(null);
  let loading = $state(false);

  /**
   * `checkUpdate` degrades a network failure into `available: false`, so a
   * successful-but-negative check is indistinguishable from a failure by payload
   * alone. Track the phase explicitly instead of inferring it.
   */
  type UpdatePhase = 'idle' | 'checking' | 'checked' | 'failed';
  let updatePhase = $state<UpdatePhase>('idle');
  let update = $state<UpdateInfo | null>(null);
  let updateError = $state<AppError | null>(null);

  async function load(): Promise<void> {
    loading = true;
    try {
      info = await diagnosticsService.get();
      loadError = null;
    } catch (cause) {
      loadError = toAppError(cause);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (info === null && loadError === null && !loading) void load();
  });

  async function retry(): Promise<void> {
    loadError = null;
    await load();
  }

  /** Clipboard access is guarded and only ever reached from a click handler. */
  async function copy(label: string, value: string): Promise<void> {
    if (!navigator.clipboard?.writeText) {
      notify.error('无法访问剪贴板', '当前环境不支持剪贴板写入');
      return;
    }
    try {
      await navigator.clipboard.writeText(value);
      notify.success(`${label} 已复制`);
    } catch (cause) {
      notify.fromError(toAppError(cause), '复制失败');
    }
  }

  async function checkUpdate(): Promise<void> {
    updatePhase = 'checking';
    updateError = null;
    try {
      update = await diagnosticsService.checkUpdate();
      updatePhase = 'checked';
      if (update.available) {
        notify.info('发现新版本', update.latestVersion ?? '');
      } else {
        notify.success('已是最新版本');
      }
    } catch (cause) {
      updateError = toAppError(cause);
      updatePhase = 'failed';
      notify.error('无法连接', '检查更新失败，请稍后重试');
    }
  }

  const paths = $derived(
    info === null
      ? []
      : [
          { label: '数据目录', value: info.dataDir },
          { label: '日志目录', value: info.logDir },
          { label: '下载目录', value: info.downloadDir }
        ]
  );
</script>

<div class="mx-auto max-w-3xl space-y-6 p-6">
  <header class="flex items-start justify-between gap-4">
    <div>
      <h1 class="text-2xl font-semibold tracking-tight">诊断信息</h1>
      <p class="text-muted-foreground mt-1 text-sm">反馈问题时请附上以下信息。</p>
    </div>
    <Button variant="outline" size="sm" disabled={loading} onclick={retry} class="shrink-0">
      <RefreshCw class="size-4" />
      刷新
    </Button>
  </header>

  {#if loadError}
    <Alert.Root variant="destructive">
      <TriangleAlert />
      <Alert.Title>无法读取诊断信息</Alert.Title>
      <Alert.Description>{loadError.message}（错误码：{loadError.code}）</Alert.Description>
      <Alert.Action>
        <Button variant="outline" size="sm" onclick={retry}>重试</Button>
      </Alert.Action>
    </Alert.Root>
  {:else if info === null}
    <Card.Root>
      <Card.Content class="space-y-4 py-6">
        {#each [0, 1, 2, 3, 4] as row (row)}
          <Skeleton class="h-5 w-full" />
        {/each}
      </Card.Content>
    </Card.Root>
  {:else}
    {#if !info.databaseReady}
      <Alert.Root variant="destructive">
        <TriangleAlert />
        <Alert.Title>数据库未就绪</Alert.Title>
        <Alert.Description>
          本地数据库无法打开或迁移失败。歌单、播放历史与下载记录将无法保存。请检查数据目录的写入权限后重启应用。
        </Alert.Description>
      </Alert.Root>
    {/if}

    <Card.Root>
      <Card.Header>
        <Card.Title>运行环境</Card.Title>
      </Card.Header>
      <Card.Content>
        <dl class="grid gap-y-3 text-sm sm:grid-cols-[8rem_1fr] sm:gap-x-4">
          <dt class="text-muted-foreground">应用版本</dt>
          <dd class="font-mono">{info.appVersion}</dd>

          <dt class="text-muted-foreground">操作系统</dt>
          <dd class="font-mono">{info.platform}</dd>

          <dt class="text-muted-foreground">架构</dt>
          <dd class="font-mono">{info.arch}</dd>

          <dt class="text-muted-foreground">数据库</dt>
          <dd>
            {#if info.databaseReady}
              <Badge>正常</Badge>
            {:else}
              <Badge variant="destructive">异常</Badge>
            {/if}
          </dd>

          {#each paths as item (item.label)}
            <dt class="text-muted-foreground">{item.label}</dt>
            <dd class="flex items-start gap-2">
              <span class="min-w-0 flex-1 font-mono text-xs break-all">{item.value}</span>
              <Button
                variant="ghost"
                size="icon"
                class="size-7 shrink-0"
                aria-label={`复制${item.label}`}
                onclick={() => copy(item.label, item.value)}
              >
                <Copy class="size-3.5" />
              </Button>
            </dd>
          {/each}
        </dl>
      </Card.Content>
    </Card.Root>

    <Card.Root>
      <Card.Header>
        <Card.Title>版本更新</Card.Title>
        <Card.Description>从配置的仓库地址查询最新发布。</Card.Description>
      </Card.Header>
      <Card.Content class="space-y-4">
        <div class="flex flex-wrap items-center gap-3">
          <Button size="sm" disabled={updatePhase === 'checking'} onclick={checkUpdate}>
            {updatePhase === 'checking' ? '检查中…' : '检查更新'}
          </Button>
          {#if updatePhase === 'idle'}
            <span class="text-muted-foreground text-sm">尚未检查</span>
          {/if}
        </div>

        {#if updatePhase === 'failed'}
          <Alert.Root variant="destructive">
            <TriangleAlert />
            <Alert.Title>无法连接</Alert.Title>
            <Alert.Description>
              未能获取版本信息，因此无法判断是否为最新版本。{updateError
                ? `（${updateError.message}）`
                : ''}
            </Alert.Description>
            <Alert.Action>
              <Button variant="outline" size="sm" onclick={checkUpdate}>重试</Button>
            </Alert.Action>
          </Alert.Root>
        {:else if updatePhase === 'checked' && update}
          <Separator />
          <dl class="grid gap-y-2 text-sm sm:grid-cols-[8rem_1fr] sm:gap-x-4">
            <dt class="text-muted-foreground">当前版本</dt>
            <dd class="font-mono">{update.currentVersion}</dd>
            <dt class="text-muted-foreground">最新版本</dt>
            <dd class="font-mono">{update.latestVersion ?? '未知'}</dd>
          </dl>
          {#if update.available}
            <div class="flex flex-wrap items-center gap-3">
              <Badge>有新版本</Badge>
              {#if update.releaseUrl}
                <Button variant="outline" size="sm" href={update.releaseUrl} target="_blank" rel="noreferrer">
                  <ExternalLink class="size-4" />
                  查看发布页
                </Button>
              {/if}
            </div>
          {:else}
            <p class="text-muted-foreground text-sm">已是最新版本。</p>
          {/if}
        {/if}
      </Card.Content>
    </Card.Root>
  {/if}
</div>
