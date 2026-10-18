<script lang="ts">
  import type { AppError, AuthStatus, QrLoginSession, QrStatus, SourceId } from '$lib/domain/contracts';
  import { authService } from '$lib/services/admin/auth-service';
  import { toAppError } from '$lib/services/errors';
  import { notify } from '$lib/stores/toast';
  import { sourceName } from '$lib/components/music/SourceBadge.svelte';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Smartphone from '@lucide/svelte/icons/smartphone';

  /** Often enough that a confirmed scan shows up promptly, without hammering the platform. */
  const POLL_INTERVAL_MS = 2000;

  let {
    source,
    open,
    onOpenChange,
    onConfirmed
  }: {
    source: SourceId;
    open: boolean;
    onOpenChange: (open: boolean) => void;
    /** Fresh status after a confirmed scan, so the caller can flip to 已配置. */
    onConfirmed: (status: AuthStatus) => void;
  } = $props();

  /** `idle` before the first request, `error` when a call itself failed. */
  type Phase = 'idle' | 'loading' | 'error' | QrStatus;

  let session = $state<QrLoginSession | null>(null);
  let phase = $state<Phase>('idle');
  let message = $state('');
  let error = $state<AppError | null>(null);
  let now = $state(Date.now());
  /** Guards against overlapping polls when one request outlives the interval. */
  let inflight = false;

  const terminal = $derived(
    phase === 'confirmed' || phase === 'expired' || phase === 'failed' || phase === 'error'
  );
  const expiredByClock = $derived(session !== null && now >= session.expiresAt * 1000);
  const polling = $derived(open && session !== null && !terminal && !expiredByClock);
  const secondsLeft = $derived(
    session === null ? 0 : Math.max(0, Math.ceil((session.expiresAt * 1000 - now) / 1000))
  );

  const hint = $derived.by(() => {
    if (phase === 'scanned') return '已扫码，请在手机上确认';
    if (phase === 'confirmed') return '登录成功，凭据已保存';
    if (phase === 'expired' || expiredByClock) return '二维码已过期，请点击刷新重新获取';
    if (phase === 'failed') return message || '扫码登录失败，请重新获取二维码';
    return message || `请使用 ${sourceName(source)} App 扫描二维码`;
  });

  async function start(): Promise<void> {
    inflight = false;
    phase = 'loading';
    error = null;
    message = '';
    session = null;
    try {
      const next = await authService.createQrLogin(source);
      session = next;
      now = Date.now();
      phase = 'pending';
    } catch (cause) {
      phase = 'error';
      error = toAppError(cause);
    }
  }

  function reset(): void {
    inflight = false;
    session = null;
    phase = 'idle';
    message = '';
    error = null;
  }

  async function poll(): Promise<void> {
    const current = session;
    if (current === null || inflight) return;
    inflight = true;
    try {
      const result = await authService.checkQrLogin(source, current.key);
      // Drop a late response belonging to a session the user already replaced.
      if (session !== current) return;
      phase = result.status;
      message = result.message;
      if (result.status === 'confirmed') {
        notify.success(`${sourceName(source)} 扫码登录成功`, 'Cookie 已保存到本机');
        onConfirmed(await authService.getStatus(source));
      }
    } catch (cause) {
      if (session !== current) return;
      phase = 'error';
      error = toAppError(cause);
    } finally {
      inflight = false;
    }
  }

  // Open/close drives the session lifecycle; closing discards it so reopening
  // always requests a fresh code.
  $effect(() => {
    if (open) {
      if (session === null && phase === 'idle') void start();
    } else if (session !== null || phase !== 'idle') {
      reset();
    }
  });

  // Teardown and every terminal state clear the timer: `polling` goes false, the
  // effect re-runs, and the cleanup below fires. No poll outlives the dialog.
  $effect(() => {
    if (!polling) return;
    const timer = setInterval(() => void poll(), POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  });

  // Separate 1s ticker so the countdown moves and `expiredByClock` can stop the
  // poll on its own once `expiresAt` passes.
  $effect(() => {
    if (session === null || terminal) return;
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });
</script>

<Dialog.Root {open} {onOpenChange}>
  <Dialog.Content class="sm:max-w-sm">
    <Dialog.Header>
      <Dialog.Title>扫码登录 {sourceName(source)}</Dialog.Title>
      <Dialog.Description>
        扫码成功后 Cookie 会自动保存到本机，无需手动粘贴。
      </Dialog.Description>
    </Dialog.Header>

    <div class="flex flex-col items-center gap-3 py-2">
      {#if phase === 'error' && error}
        <Alert.Root variant="destructive">
          <TriangleAlert />
          <Alert.Title>扫码登录失败</Alert.Title>
          <Alert.Description>{error.message}（错误码：{error.code}）</Alert.Description>
        </Alert.Root>
      {:else if session === null}
        <Skeleton class="size-48 rounded-md" />
      {:else}
        <div class="relative">
          <img
            src={session.imageUrl}
            alt={`${sourceName(source)} 扫码登录二维码`}
            class="size-48 rounded-md border bg-white p-1"
            class:opacity-30={phase === 'expired' || expiredByClock || phase === 'confirmed'}
          />
          {#if phase === 'scanned'}
            <div
              class="bg-background/80 absolute inset-0 flex flex-col items-center justify-center gap-2 rounded-md"
            >
              <Smartphone class="size-8" />
              <span class="text-sm font-medium">已扫码</span>
            </div>
          {:else if phase === 'confirmed'}
            <div
              class="bg-background/80 absolute inset-0 flex items-center justify-center rounded-md"
            >
              <CircleCheck class="size-10 text-emerald-600" />
            </div>
          {/if}
        </div>
      {/if}

      <p
        class="text-center text-sm"
        class:text-muted-foreground={phase !== 'scanned' && phase !== 'confirmed'}
        aria-live="polite"
      >
        {hint}
      </p>

      {#if session !== null && !terminal && !expiredByClock}
        <p class="text-muted-foreground text-xs">二维码 {secondsLeft} 秒后过期</p>
      {/if}
    </div>

    <Dialog.Footer>
      {#if phase === 'confirmed'}
        <Button size="sm" onclick={() => onOpenChange(false)}>完成</Button>
      {:else}
        <Button variant="outline" size="sm" onclick={() => onOpenChange(false)}>关闭</Button>
        <Button
          size="sm"
          disabled={phase === 'loading'}
          onclick={() => void start()}
        >
          <RefreshCw class="size-3.5" />
          {phase === 'loading' ? '获取中…' : '刷新二维码'}
        </Button>
      {/if}
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
