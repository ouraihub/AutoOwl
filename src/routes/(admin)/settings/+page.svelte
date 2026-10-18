<script lang="ts">
  import type { AppSettings } from '$lib/domain/contracts';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { notify } from '$lib/stores/toast';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { Switch } from '$lib/components/ui/switch/index.js';
  import { Slider } from '$lib/components/ui/slider/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import Save from '@lucide/svelte/icons/save';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';

  /** Editable copy; `null` until the first successful load. */
  let form = $state<AppSettings | null>(null);
  /** The last persisted snapshot, for dirty tracking and discard. */
  let baseline = $state<string>('');
  let saving = $state(false);

  /**
   * The backend clamps `downloadConcurrency` and `pageSize`, so the reply is the
   * source of truth for both the form and the dirty baseline.
   */
  function adopt(settings: AppSettings): void {
    form = { ...settings };
    baseline = JSON.stringify(settings);
  }

  $effect(() => {
    if (settingsStore.settings === null && !settingsStore.loading) {
      void settingsStore.load();
    }
  });

  // Adopt whatever the store holds whenever it changes identity (load or save).
  $effect(() => {
    const settings = settingsStore.settings;
    if (settings !== null && JSON.stringify(settings) !== baseline) {
      adopt(settings);
    }
  });

  const dirty = $derived(form !== null && JSON.stringify(form) !== baseline);

  async function save(): Promise<void> {
    if (form === null) return;
    saving = true;
    const next = { ...form };
    await settingsStore.save(next);
    saving = false;
    const error = settingsStore.error;
    if (error) {
      notify.fromError(error, '保存设置失败');
      return;
    }
    if (settingsStore.settings) adopt(settingsStore.settings);
    notify.success('设置已保存', '超出范围的数值已由后端修正');
  }

  function discard(): void {
    if (settingsStore.settings) adopt(settingsStore.settings);
  }

  async function retry(): Promise<void> {
    settingsStore.clearError();
    await settingsStore.load();
  }
</script>

<div class="mx-auto max-w-3xl space-y-6 p-6">
  <header class="flex items-start justify-between gap-4">
    <div>
      <h1 class="text-2xl font-semibold tracking-tight">设置</h1>
      <p class="text-muted-foreground mt-1 text-sm">下载、搜索、更新与视频导出的全局配置。</p>
    </div>
    <div class="flex shrink-0 gap-2">
      <Button variant="outline" size="sm" disabled={!dirty || saving} onclick={discard}>
        <RotateCcw class="size-4" />
        放弃更改
      </Button>
      <Button size="sm" disabled={!dirty || saving} onclick={save}>
        <Save class="size-4" />
        {saving ? '保存中…' : '保存'}
      </Button>
    </div>
  </header>

  {#if settingsStore.error && form === null}
    <Alert.Root variant="destructive">
      <TriangleAlert />
      <Alert.Title>无法加载设置</Alert.Title>
      <Alert.Description>
        {settingsStore.error.message}（错误码：{settingsStore.error.code}）
      </Alert.Description>
      <Alert.Action>
        <Button variant="outline" size="sm" onclick={retry}>重试</Button>
      </Alert.Action>
    </Alert.Root>
  {:else if form === null}
    <div class="space-y-4">
      {#each [0, 1, 2, 3] as block (block)}
        <Card.Root>
          <Card.Header>
            <Skeleton class="h-5 w-24" />
          </Card.Header>
          <Card.Content class="space-y-4">
            <Skeleton class="h-9 w-full" />
            <Skeleton class="h-9 w-2/3" />
          </Card.Content>
        </Card.Root>
      {/each}
    </div>
  {:else}
    {#if settingsStore.error}
      <Alert.Root variant="destructive">
        <TriangleAlert />
        <Alert.Title>上次操作失败</Alert.Title>
        <Alert.Description>
          {settingsStore.error.message}（错误码：{settingsStore.error.code}）
        </Alert.Description>
        <Alert.Action>
          <Button variant="outline" size="sm" onclick={() => settingsStore.clearError()}>忽略</Button>
        </Alert.Action>
      </Alert.Root>
    {/if}

    <Card.Root>
      <Card.Header>
        <Card.Title>下载</Card.Title>
        <Card.Description>文件落盘位置、命名方式与并发。</Card.Description>
      </Card.Header>
      <Card.Content class="space-y-6">
        <div class="space-y-2">
          <Label for="download-dir">下载目录</Label>
          <Input id="download-dir" bind:value={form.downloadDir} placeholder="例如 D:\Music" />
        </div>

        <div class="space-y-2">
          <Label for="filename-template">文件名模板</Label>
          <Input
            id="filename-template"
            bind:value={form.downloadFilenameTemplate}
            placeholder="{'{artist}'}/{'{name}'}.{'{ext}'}"
          />
          <p class="text-muted-foreground text-xs">
            支持的占位符：<code class="text-foreground">{'{name}'}</code>
            曲名、<code class="text-foreground">{'{artist}'}</code>
            首位歌手、<code class="text-foreground">{'{album}'}</code>
            专辑、<code class="text-foreground">{'{source}'}</code>
            来源平台、<code class="text-foreground">{'{id}'}</code>
            平台曲目 ID、<code class="text-foreground">{'{ext}'}</code>
            扩展名。可用 <code class="text-foreground">/</code> 分隔子目录。
          </p>
        </div>

        <div class="flex items-center justify-between gap-4">
          <div class="space-y-1">
            <Label for="embed-download">写入元数据</Label>
            <p class="text-muted-foreground text-xs">下载后嵌入封面、标题与歌手信息（需要 ffmpeg）。</p>
          </div>
          <Switch id="embed-download" bind:checked={form.embedDownload} />
        </div>

        <div class="space-y-3">
          <div class="flex items-center justify-between">
            <Label for="download-concurrency">下载并发数</Label>
            <span class="text-muted-foreground text-sm tabular-nums">{form.downloadConcurrency}</span>
          </div>
          <Slider
            id="download-concurrency"
            type="single"
            min={1}
            max={5}
            step={1}
            bind:value={form.downloadConcurrency}
          />
          <p class="text-muted-foreground text-xs">允许范围 1 - 5。</p>
        </div>

        <div class="flex items-center justify-between gap-4">
          <div class="space-y-1">
            <Label for="auto-cache">播放时自动缓存</Label>
            <p class="text-muted-foreground text-xs">试听过的曲目自动保存到下载目录。</p>
          </div>
          <Switch id="auto-cache" bind:checked={form.autoCacheOnPlay} />
        </div>
      </Card.Content>
    </Card.Root>

    <Card.Root>
      <Card.Header>
        <Card.Title>搜索与播放</Card.Title>
        <Card.Description>结果分页、失效音源处理与歌词窗口。</Card.Description>
      </Card.Header>
      <Card.Content class="space-y-6">
        <div class="space-y-3">
          <div class="flex items-center justify-between">
            <Label for="page-size">每页结果数</Label>
            <span class="text-muted-foreground text-sm tabular-nums">{form.pageSize}</span>
          </div>
          <Slider id="page-size" type="single" min={10} max={100} step={10} bind:value={form.pageSize} />
          <p class="text-muted-foreground text-xs">允许范围 10 - 100。</p>
        </div>

        <div class="flex items-center justify-between gap-4">
          <div class="space-y-1">
            <Label for="auto-switch">自动切换失效音源</Label>
            <p class="text-muted-foreground text-xs">当前音源不可播放时，自动尝试其他平台的同名曲目。</p>
          </div>
          <Switch id="auto-switch" bind:checked={form.autoSwitchInvalidSources} />
        </div>

        <div class="flex items-center justify-between gap-4">
          <div class="space-y-1">
            <Label for="disable-lyrics">关闭悬浮歌词</Label>
            <p class="text-muted-foreground text-xs">不再显示桌面悬浮歌词窗口。</p>
          </div>
          <Switch id="disable-lyrics" bind:checked={form.disableFloatingLyrics} />
        </div>
      </Card.Content>
    </Card.Root>

    <Card.Root>
      <Card.Header>
        <Card.Title>更新</Card.Title>
        <Card.Description>版本检查与 GitHub 加速代理。</Card.Description>
      </Card.Header>
      <Card.Content class="space-y-6">
        <div class="flex items-center justify-between gap-4">
          <div class="space-y-1">
            <Label for="auto-check-update">启动时检查更新</Label>
            <p class="text-muted-foreground text-xs">连接失败不会影响正常使用。</p>
          </div>
          <Switch id="auto-check-update" bind:checked={form.autoCheckUpdate} />
        </div>

        <div class="space-y-2">
          <Label for="update-repo">更新仓库地址</Label>
          <Input id="update-repo" bind:value={form.updateRepoUrl} placeholder="https://github.com/…" />
        </div>

        <div class="flex items-center justify-between gap-4">
          <div class="space-y-1">
            <Label for="proxy-enabled">启用 GitHub 代理</Label>
            <p class="text-muted-foreground text-xs">国内网络下加速版本检查与下载。</p>
          </div>
          <Switch id="proxy-enabled" bind:checked={form.githubProxyEnabled} />
        </div>

        <div class="space-y-2">
          <Label for="proxy-url" class={form.githubProxyEnabled ? '' : 'text-muted-foreground'}>
            代理地址
          </Label>
          <Input
            id="proxy-url"
            bind:value={form.githubProxyUrl}
            disabled={!form.githubProxyEnabled}
            placeholder="https://ghproxy.example/"
          />
          {#if !form.githubProxyEnabled}
            <p class="text-muted-foreground text-xs">先启用 GitHub 代理后才能编辑。</p>
          {/if}
        </div>
      </Card.Content>
    </Card.Root>

    <Card.Root>
      <Card.Header>
        <Card.Title>视频导出</Card.Title>
        <Card.Description>生成视频时允许替换的素材，以及是否导出成品。</Card.Description>
      </Card.Header>
      <Card.Content class="space-y-6">
        <div class="flex items-center justify-between gap-4">
          <Label for="vg-cover">允许更换封面</Label>
          <Switch id="vg-cover" bind:checked={form.vgChangeCover} />
        </div>
        <div class="flex items-center justify-between gap-4">
          <Label for="vg-audio">允许更换音频</Label>
          <Switch id="vg-audio" bind:checked={form.vgChangeAudio} />
        </div>
        <div class="flex items-center justify-between gap-4">
          <Label for="vg-lyric">允许更换歌词</Label>
          <Switch id="vg-lyric" bind:checked={form.vgChangeLyric} />
        </div>
        <div class="flex items-center justify-between gap-4">
          <div class="space-y-1">
            <Label for="vg-export">导出视频文件</Label>
            <p class="text-muted-foreground text-xs">需要 ffmpeg，可在“媒体工具”页确认。</p>
          </div>
          <Switch id="vg-export" bind:checked={form.vgExportVideo} />
        </div>
      </Card.Content>
    </Card.Root>
  {/if}
</div>
