<script lang="ts">
  import type { Component, Snippet } from 'svelte';
  import { page } from '$app/state';
  import { cn } from '$lib/utils';
  import * as ScrollArea from '$lib/components/ui/scroll-area';
  import { Separator } from '$lib/components/ui/separator';
  import SearchIcon from '@lucide/svelte/icons/search';
  import ListMusicIcon from '@lucide/svelte/icons/list-music';
  import DiscIcon from '@lucide/svelte/icons/disc-3';
  import HeartIcon from '@lucide/svelte/icons/heart';
  import FolderIcon from '@lucide/svelte/icons/folder-open';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import HistoryIcon from '@lucide/svelte/icons/history';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import CookieIcon from '@lucide/svelte/icons/cookie';
  import WrenchIcon from '@lucide/svelte/icons/wrench';
  import StethoscopeIcon from '@lucide/svelte/icons/stethoscope';
  import InfoIcon from '@lucide/svelte/icons/info';
  import logo from '$lib/assets/logo.png';
  import BotIcon from '@lucide/svelte/icons/bot';
  import ScrollTextIcon from '@lucide/svelte/icons/scroll-text';
  import MessagesSquareIcon from '@lucide/svelte/icons/messages-square';

  type NavItem = {
    href: string;
    label: string;
    icon: Component<{ class?: string }>;
  };

  let {
    children,
    player,
    toolbar
  }: {
    children: Snippet;
    /** Rendered into the fixed-height region reserved at the bottom of the shell. */
    player?: Snippet;
    /** Rendered right-aligned in the top bar. */
    toolbar?: Snippet;
  } = $props();

  const libraryNav: NavItem[] = [
    { href: '/search', label: '搜索', icon: SearchIcon },
    { href: '/chat', label: 'AI 对话', icon: MessagesSquareIcon },
    { href: '/collections', label: '歌单', icon: ListMusicIcon },
    { href: '/album', label: '专辑', icon: DiscIcon },
    { href: '/my-collections', label: '我的歌单', icon: HeartIcon },
    { href: '/local-music', label: '本地音乐', icon: FolderIcon },
    { href: '/downloads', label: '下载记录', icon: DownloadIcon },
    { href: '/history', label: '播放历史', icon: HistoryIcon }
  ];

  const adminNav: NavItem[] = [
    { href: '/settings', label: '系统设置', icon: SettingsIcon },
    { href: '/settings/accounts', label: '账号 Cookie', icon: CookieIcon },
    { href: '/settings/media', label: '媒体工具', icon: WrenchIcon },
    { href: '/settings/ai', label: 'AI 接入', icon: BotIcon },
    { href: '/settings/diagnostics', label: '诊断', icon: StethoscopeIcon },
    { href: '/settings/logs', label: '日志', icon: ScrollTextIcon },
    { href: '/about', label: '关于', icon: InfoIcon }
  ];

  /**
   * `/settings` is a prefix of `/settings/accounts`, so a plain `startsWith`
   * would light up two entries at once. Only the deepest matching route wins.
   */
  const activeHref = $derived.by(() => {
    const pathname = page.url.pathname;
    let best = '';
    for (const { href } of [...libraryNav, ...adminNav]) {
      const matches = pathname === href || pathname.startsWith(`${href}/`);
      if (matches && href.length > best.length) best = href;
    }
    return best;
  });

  const activeLabel = $derived(
    [...libraryNav, ...adminNav].find((item) => item.href === activeHref)?.label ?? 'AutoOwl'
  );
</script>

<div class="flex h-screen w-full overflow-hidden bg-background text-foreground">
  <aside
    class="flex w-56 shrink-0 flex-col gap-2 border-r border-border bg-sidebar text-sidebar-foreground"
    aria-label="主导航"
  >
    <a href="/search" class="flex h-14 shrink-0 items-center gap-2 px-4 font-semibold tracking-tight">
      <img src={logo} alt="" class="size-6 rounded" />
      AutoOwl
    </a>

    <ScrollArea.Root class="min-h-0 flex-1">
      <nav class="flex flex-col gap-1 px-2 pb-4">
        {#each libraryNav as item (item.href)}
          {@const Icon = item.icon}
          <a
            href={item.href}
            aria-current={activeHref === item.href ? 'page' : undefined}
            class={cn(
              'flex items-center gap-2 rounded-md px-3 py-2 text-sm transition-colors',
              'hover:bg-sidebar-accent hover:text-sidebar-accent-foreground',
              'focus-visible:ring-2 focus-visible:ring-sidebar-ring focus-visible:outline-none',
              activeHref === item.href
                ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground'
                : 'text-muted-foreground'
            )}
          >
            <Icon class="size-4 shrink-0" />
            <span class="truncate">{item.label}</span>
          </a>
        {/each}

        <Separator class="my-2 bg-sidebar-border" />
        <p class="px-3 pb-1 text-xs font-medium text-muted-foreground">管理</p>

        {#each adminNav as item (item.href)}
          {@const Icon = item.icon}
          <a
            href={item.href}
            aria-current={activeHref === item.href ? 'page' : undefined}
            class={cn(
              'flex items-center gap-2 rounded-md px-3 py-2 text-sm transition-colors',
              'hover:bg-sidebar-accent hover:text-sidebar-accent-foreground',
              'focus-visible:ring-2 focus-visible:ring-sidebar-ring focus-visible:outline-none',
              activeHref === item.href
                ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground'
                : 'text-muted-foreground'
            )}
          >
            <Icon class="size-4 shrink-0" />
            <span class="truncate">{item.label}</span>
          </a>
        {/each}
      </nav>
    </ScrollArea.Root>
  </aside>

  <div class="flex min-w-0 flex-1 flex-col">
    <header
      class="flex h-14 shrink-0 items-center gap-3 border-b border-border bg-background px-6"
    >
      <h1 class="truncate text-sm font-medium">{activeLabel}</h1>
      {#if toolbar}
        <div class="ml-auto flex items-center gap-2">{@render toolbar()}</div>
      {/if}
    </header>

    <main class="min-h-0 flex-1 overflow-y-auto">
      {@render children()}
    </main>

    {#if player}
      <div class="h-22 shrink-0 border-t border-border bg-card">
        {@render player()}
      </div>
    {/if}
  </div>
</div>
