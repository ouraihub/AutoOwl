<script lang="ts">
  import '../app.css';
  import type { Snippet } from 'svelte';
  import { createDefaultNotify, createI18n, KitNotifier, setKitContext } from '@ouraihub/aether-ui';
  import { createLogger } from '@ouraihub/aether-ui/logs';
  import { createTauriPorts } from '@ouraihub/aether-ui/tauri';
  import { invoke } from '@tauri-apps/api/core';
  import AppShell from '$lib/components/layout/AppShell.svelte';
  import PlayerBar from '$lib/components/music/PlayerBar.svelte';

  let { children }: { children: Snippet } = $props();

  // Aether kit: AI chat, model providers, MCP server settings, log viewer. Its
  // notifier owns the single <Toaster />, so app toasts land there too.
  const ports = createTauriPorts({ providers: true, chat: true, chatHistory: true, mcpServer: true });
  const i18n = createI18n('zh-CN', {
    'music.scope.read': '搜索歌曲、查看播放状态',
    'music.scope.control': '播放、暂停、切歌'
  });
  const notify = createDefaultNotify();
  setKitContext({ ports, i18n, notify, logger: createLogger(ports.logs) });

  // The WebView ignores `<a target="_blank">` (no new windows), so every external
  // link (ours and aether-ui's, e.g. 获取密钥) goes to the system browser instead.
  function openExternal(event: MouseEvent): void {
    const link = (event.target as Element | null)?.closest('a[target="_blank"]');
    const href = link?.getAttribute('href') ?? '';
    if (!/^https?:\/\//i.test(href)) return;
    event.preventDefault();
    void invoke('plugin:opener|open_url', { url: href });
  }
</script>

<svelte:document onclick={openExternal} />

<AppShell {children}>
  {#snippet player()}<PlayerBar />{/snippet}
</AppShell>

<KitNotifier {notify} {i18n} />
