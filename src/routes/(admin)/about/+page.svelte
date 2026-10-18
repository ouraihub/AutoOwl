<script lang="ts">
  import type { AppError, Diagnostics, SourceDescriptor } from '$lib/domain/contracts';
  import { diagnosticsService } from '$lib/services/admin/diagnostics-service';
  import { searchService } from '$lib/services/user/search-service';
  import { toAppError } from '$lib/services/errors';
  import SourceBadge from '$lib/components/music/SourceBadge.svelte';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import Check from '@lucide/svelte/icons/check';
  import Minus from '@lucide/svelte/icons/minus';
  import Info from '@lucide/svelte/icons/info';

  let info = $state<Diagnostics | null>(null);
  let sources = $state<SourceDescriptor[] | null>(null);
  let loadError = $state<AppError | null>(null);
  let loading = $state(false);

  async function load(): Promise<void> {
    loading = true;
    try {
      const [diagnostics, descriptors] = await Promise.all([
        diagnosticsService.get(),
        searchService.getSourceCapabilities()
      ]);
      info = diagnostics;
      sources = descriptors;
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

  const columns: { key: keyof CapabilityFlags; label: string }[] = [
    { key: 'songSearch', label: '搜索' },
    { key: 'playlistSearch', label: '歌单' },
    { key: 'albumSearch', label: '专辑' },
    { key: 'lyrics', label: '歌词' },
    { key: 'qrLogin', label: '扫码登录' }
  ];

  type CapabilityFlags = {
    songSearch: boolean;
    playlistSearch: boolean;
    albumSearch: boolean;
    lyrics: boolean;
    qrLogin: boolean;
  };
</script>

<div class="mx-auto max-w-3xl space-y-6 p-6">
  <header>
    <h1 class="text-2xl font-semibold tracking-tight">关于</h1>
  </header>

  <Card.Root>
    <Card.Header>
      <Card.Title class="text-xl">AutoOwl</Card.Title>
      <Card.Description>聚合音乐搜索与下载工具</Card.Description>
    </Card.Header>
    <Card.Content class="space-y-3 text-sm">
      <p class="text-muted-foreground">
        跨多个音乐平台聚合搜索单曲、歌单与专辑，支持在线试听、歌词显示、批量下载与元数据写入。
      </p>
      <div class="flex items-center gap-2">
        <span class="text-muted-foreground">版本</span>
        {#if info}
          <code class="font-mono">{info.appVersion}</code>
        {:else if loadError}
          <span class="text-muted-foreground">未知</span>
        {:else}
          <Skeleton class="h-4 w-16" />
        {/if}
      </div>
    </Card.Content>
  </Card.Root>

  {#if loadError}
    <Alert.Root variant="destructive">
      <TriangleAlert />
      <Alert.Title>无法加载平台信息</Alert.Title>
      <Alert.Description>{loadError.message}（错误码：{loadError.code}）</Alert.Description>
      <Alert.Action>
        <Button variant="outline" size="sm" onclick={retry}>重试</Button>
      </Alert.Action>
    </Alert.Root>
  {/if}

  <Card.Root>
    <Card.Header>
      <Card.Title>支持的平台</Card.Title>
      <Card.Description>各平台可用能力由后端探测得出。</Card.Description>
    </Card.Header>
    <Card.Content>
      {#if sources === null && !loadError}
        <div class="space-y-3">
          {#each [0, 1, 2, 3, 4] as row (row)}
            <Skeleton class="h-8 w-full" />
          {/each}
        </div>
      {:else if sources && sources.length > 0}
        <Table.Root>
          <Table.Header>
            <Table.Row>
              <Table.Head>平台</Table.Head>
              {#each columns as column (column.key)}
                <Table.Head class="text-center">{column.label}</Table.Head>
              {/each}
              <Table.Head>音质</Table.Head>
            </Table.Row>
          </Table.Header>
          <Table.Body>
            {#each sources as descriptor (descriptor.id)}
              <Table.Row>
                <Table.Cell>
                  <SourceBadge source={descriptor.id} />
                </Table.Cell>
                {#each columns as column (column.key)}
                  <Table.Cell class="text-center">
                    {#if descriptor.capabilities[column.key]}
                      <Check class="text-primary mx-auto size-4" aria-label="支持" />
                    {:else}
                      <Minus class="text-muted-foreground mx-auto size-4" aria-label="不支持" />
                    {/if}
                  </Table.Cell>
                {/each}
                <Table.Cell class="text-muted-foreground text-xs">
                  {descriptor.capabilities.qualities.length > 0
                    ? descriptor.capabilities.qualities.join(' / ')
                    : '默认'}
                </Table.Cell>
              </Table.Row>
            {/each}
          </Table.Body>
        </Table.Root>
      {:else if sources}
        <p class="text-muted-foreground py-6 text-center text-sm">后端未报告任何可用平台。</p>
      {/if}
    </Card.Content>
  </Card.Root>

  <Alert.Root>
    <Info />
    <Alert.Title>使用声明</Alert.Title>
    <Alert.Description>
      本工具仅供个人学习与研究使用，所有音频与歌词内容版权归各平台及权利人所有。
      请勿用于商业用途或公开传播，下载的内容请在 24 小时内自行删除。使用者需自行承担因违规使用产生的责任。
    </Alert.Description>
  </Alert.Root>
</div>
