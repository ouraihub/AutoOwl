<script lang="ts">
  import type { StoragePaths } from '$lib/domain/contracts';
  import { settingsService } from '$lib/services/admin/settings-service';
  import { toAppError } from '$lib/services/errors';
  import { notify } from '$lib/stores/toast';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Info from '@lucide/svelte/icons/info';

  let paths = $state<StoragePaths | null>(null);
  /** Editable choices; empty = default. */
  let dataDir = $state('');
  let logsDir = $state('');
  let saving = $state(false);
  let loadError = $state<string | null>(null);

  function adopt(next: StoragePaths): void {
    paths = next;
    dataDir = next.dataDirOverride;
    logsDir = next.logsDirOverride;
  }

  async function load(): Promise<void> {
    try {
      adopt(await settingsService.storagePaths());
      loadError = null;
    } catch (cause) {
      loadError = toAppError(cause).message;
    }
  }

  $effect(() => {
    void load();
  });

  const dirty = $derived(
    paths !== null && (dataDir.trim() !== paths.dataDirOverride || logsDir.trim() !== paths.logsDirOverride)
  );

  async function pick(current: string, fallback: string): Promise<string | null> {
    try {
      return await settingsService.pickFolder(current || fallback);
    } catch (cause) {
      notify.fromError(toAppError(cause), '无法打开文件夹选择');
      return null;
    }
  }

  async function save(): Promise<void> {
    saving = true;
    try {
      adopt(await settingsService.setStoragePaths(dataDir.trim(), logsDir.trim()));
      notify.success('存储位置已保存', '重启后生效');
    } catch (cause) {
      notify.fromError(toAppError(cause), '保存存储位置失败');
    } finally {
      saving = false;
    }
  }

  async function restart(): Promise<void> {
    try {
      await settingsService.restart();
    } catch (cause) {
      notify.fromError(toAppError(cause), '重启失败');
    }
  }
</script>

<Card.Root>
  <Card.Header>
    <Card.Title>存储位置</Card.Title>
    <Card.Description>数据库、模型服务、对话历史、MCP 令牌、ffmpeg 与日志存放的位置。</Card.Description>
  </Card.Header>
  <Card.Content class="space-y-6">
    {#if loadError}
      <Alert.Root variant="destructive">
        <Info />
        <Alert.Title>无法读取存储位置</Alert.Title>
        <Alert.Description>{loadError}</Alert.Description>
        <Alert.Action>
          <Button variant="outline" size="sm" onclick={load}>重试</Button>
        </Alert.Action>
      </Alert.Root>
    {:else if paths === null}
      <Skeleton class="h-9 w-full" />
      <Skeleton class="h-9 w-full" />
    {:else}
      {@const current = paths}
      <div class="space-y-2">
        <Label for="data-dir">数据目录</Label>
        <div class="flex gap-2">
          <Input id="data-dir" bind:value={dataDir} placeholder={current.defaultDataDir} />
          <Button
            variant="outline"
            size="icon"
            aria-label="选择数据目录"
            onclick={async () => {
              const folder = await pick(dataDir, current.dataDir);
              if (folder) dataDir = folder;
            }}
          >
            <FolderOpen class="size-4" />
          </Button>
        </div>
        <p class="text-muted-foreground text-xs">
          正在使用：<code class="text-foreground">{current.dataDir}</code>。留空即默认（程序目录下的 data）。
          只切换位置、不搬数据：新目录从空开始，需要旧数据请自行把原目录内容拷过去。
        </p>
      </div>

      <div class="space-y-2">
        <Label for="logs-dir">日志目录</Label>
        <div class="flex gap-2">
          <Input id="logs-dir" bind:value={logsDir} placeholder="默认：数据目录下的 logs" />
          <Button
            variant="outline"
            size="icon"
            aria-label="选择日志目录"
            onclick={async () => {
              const folder = await pick(logsDir, current.logsDir);
              if (folder) logsDir = folder;
            }}
          >
            <FolderOpen class="size-4" />
          </Button>
        </div>
        <p class="text-muted-foreground text-xs">
          正在使用：<code class="text-foreground">{current.logsDir}</code>
        </p>
      </div>

      {#if current.restartRequired}
        <Alert.Root>
          <RefreshCw />
          <Alert.Title>重启后生效</Alert.Title>
          <Alert.Description>新的存储位置已保存，重启 AutoOwl 后开始使用。</Alert.Description>
          <Alert.Action>
            <Button size="sm" onclick={restart}>立即重启</Button>
          </Alert.Action>
        </Alert.Root>
      {/if}

      <div class="flex justify-end">
        <Button size="sm" disabled={!dirty || saving} onclick={save}>
          {saving ? '保存中…' : '保存存储位置'}
        </Button>
      </div>
    {/if}
  </Card.Content>
</Card.Root>
