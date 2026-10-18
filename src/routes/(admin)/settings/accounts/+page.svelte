<script lang="ts">
  import type { AppError, AuthStatus, SourceId } from '$lib/domain/contracts';
  import { authService } from '$lib/services/admin/auth-service';
  import { toAppError } from '$lib/services/errors';
  import { notify } from '$lib/stores/toast';
  import SourceBadge from '$lib/components/music/SourceBadge.svelte';
  import { sourceName } from '$lib/components/music/SourceBadge.svelte';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import Info from '@lucide/svelte/icons/info';
  import QrCode from '@lucide/svelte/icons/qr-code';
  import QrLoginDialog from '$lib/components/admin/QrLoginDialog.svelte';

  let statuses = $state<AuthStatus[] | null>(null);
  let loadError = $state<AppError | null>(null);
  let loading = $state(false);

  /**
   * Draft cookie text per source. Never seeded from the backend: `AuthStatus`
   * carries only the `configured` flag, and a stored cookie must never reach the
   * DOM.
   */
  let drafts = $state<Record<string, string>>({});
  let busy = $state<string | null>(null);
  let confirmClear = $state<SourceId | null>(null);
  /** Source whose QR dialog is open; `null` keeps every poll torn down. */
  let qrSource = $state<SourceId | null>(null);

  async function load(): Promise<void> {
    loading = true;
    try {
      statuses = await authService.listStatus();
      loadError = null;
    } catch (cause) {
      loadError = toAppError(cause);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (statuses === null && loadError === null && !loading) void load();
  });

  /** Replace one row in place so untouched drafts survive a mutation. */
  function apply(next: AuthStatus): void {
    if (statuses === null) return;
    statuses = statuses.map((item) => (item.source === next.source ? next : item));
  }

  async function saveCookie(source: SourceId): Promise<void> {
    const cookie = (drafts[source] ?? '').trim();
    if (cookie.length === 0) {
      notify.error('请先粘贴 Cookie', `${sourceName(source)} 的输入框为空`);
      return;
    }
    busy = source;
    try {
      apply(await authService.saveCookie(source, cookie));
      drafts[source] = '';
      notify.success(`${sourceName(source)} Cookie 已保存`);
    } catch (cause) {
      notify.fromError(toAppError(cause), '保存 Cookie 失败');
    } finally {
      busy = null;
    }
  }

  async function clearCookie(source: SourceId): Promise<void> {
    busy = source;
    confirmClear = null;
    try {
      apply(await authService.clearCookie(source));
      drafts[source] = '';
      notify.success(`${sourceName(source)} Cookie 已清除`);
    } catch (cause) {
      notify.fromError(toAppError(cause), '清除 Cookie 失败');
    } finally {
      busy = null;
    }
  }

  async function retry(): Promise<void> {
    loadError = null;
    await load();
  }
</script>

<div class="mx-auto max-w-3xl space-y-6 p-6">
  <header>
    <h1 class="text-2xl font-semibold tracking-tight">账号与 Cookie</h1>
    <p class="text-muted-foreground mt-1 text-sm">
      部分平台需要登录后的 Cookie 才能搜索完整曲库或获取高音质音频。
    </p>
  </header>

  <Alert.Root>
    <Info />
    <Alert.Title>如何获取 Cookie</Alert.Title>
    <Alert.Description>
      在浏览器中登录对应平台，按 F12 打开开发者工具，切换到「网络 / Network」，刷新页面后点击任意请求，
      在「请求标头 / Request Headers」中复制 <code class="text-foreground">Cookie</code> 整行内容粘贴到下方。
      Cookie 仅保存在本机，保存后不会再回显。
    </Alert.Description>
  </Alert.Root>

  {#if loadError}
    <Alert.Root variant="destructive">
      <TriangleAlert />
      <Alert.Title>无法加载账号状态</Alert.Title>
      <Alert.Description>{loadError.message}（错误码：{loadError.code}）</Alert.Description>
      <Alert.Action>
        <Button variant="outline" size="sm" onclick={retry}>重试</Button>
      </Alert.Action>
    </Alert.Root>
  {:else if statuses === null}
    <div class="space-y-4">
      {#each [0, 1, 2] as block (block)}
        <Card.Root>
          <Card.Header><Skeleton class="h-5 w-32" /></Card.Header>
          <Card.Content class="space-y-3">
            <Skeleton class="h-20 w-full" />
            <Skeleton class="h-9 w-40" />
          </Card.Content>
        </Card.Root>
      {/each}
    </div>
  {:else if statuses.length === 0}
    <Card.Root>
      <Card.Content class="text-muted-foreground py-10 text-center text-sm">
        当前没有支持 Cookie 登录的平台。
      </Card.Content>
    </Card.Root>
  {:else}
    <div class="space-y-4">
      {#each statuses as status (status.source)}
        <Card.Root>
          <Card.Header>
            <Card.Title class="flex flex-wrap items-center gap-2">
              <SourceBadge source={status.source} />
              <span>{sourceName(status.source)}</span>
              {#if status.configured}
                <Badge>已配置</Badge>
              {:else}
                <Badge variant="outline" class="text-muted-foreground">未配置</Badge>
              {/if}
              {#if status.qrSupported}
                <Badge variant="outline" class="text-muted-foreground gap-1">
                  <QrCode class="size-3" />
                  支持扫码
                </Badge>
              {/if}
            </Card.Title>
          </Card.Header>
          <Card.Content class="space-y-3">
            <div class="space-y-2">
              <Label for={`cookie-${status.source}`}>
                {status.configured ? '替换 Cookie' : '粘贴 Cookie'}
              </Label>
              <Textarea
                id={`cookie-${status.source}`}
                rows={3}
                class="font-mono text-xs"
                placeholder="例如 uid=123; token=abc; …"
                bind:value={
                  () => drafts[status.source] ?? '',
                  (next: string) => (drafts[status.source] = next)
                }
              />
            </div>
            <div class="flex flex-wrap gap-2">
              <Button
                size="sm"
                disabled={busy === status.source || (drafts[status.source] ?? '').trim().length === 0}
                onclick={() => saveCookie(status.source)}
              >
                {busy === status.source ? '处理中…' : '保存'}
              </Button>
              {#if status.configured}
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy === status.source}
                  onclick={() => (confirmClear = status.source)}
                >
                  清除
                </Button>
              {/if}
              {#if status.qrSupported}
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy === status.source}
                  onclick={() => (qrSource = status.source)}
                >
                  <QrCode class="size-3.5" />
                  扫码登录
                </Button>
              {/if}
            </div>
          </Card.Content>
        </Card.Root>
      {/each}
    </div>
  {/if}
</div>

<Dialog.Root
  open={confirmClear !== null}
  onOpenChange={(open) => {
    if (!open) confirmClear = null;
  }}
>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>清除 Cookie</Dialog.Title>
      <Dialog.Description>
        将删除 {confirmClear ? sourceName(confirmClear) : ''} 已保存的登录凭据，该平台会退回到未登录状态。此操作无法撤销。
      </Dialog.Description>
    </Dialog.Header>
    <Dialog.Footer>
      <Button variant="outline" size="sm" onclick={() => (confirmClear = null)}>取消</Button>
      <Button
        variant="destructive"
        size="sm"
        onclick={() => {
          if (confirmClear) void clearCookie(confirmClear);
        }}
      >
        确认清除
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

{#if qrSource}{#key qrSource}
  <QrLoginDialog
    source={qrSource}
    open={qrSource !== null}
    onOpenChange={(open) => {
      if (!open) qrSource = null;
    }}
    onConfirmed={apply}
  />
{/key}{/if}
