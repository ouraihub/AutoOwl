<script lang="ts">
  import type {
    AddOutcome,
    Collection,
    CollectionKind,
    Song,
    SourceDescriptor,
    SourceFailure,
    SourceId,
    StoredCollection
  } from '$lib/domain/contracts';
  import { searchService } from '$lib/services/user/search-service';
  import { collectionService } from '$lib/services/user/collection-service';
  import { downloadService } from '$lib/services/user/download-service';
  import { localMusicService } from '$lib/services/user/local-music-service';
  import { toAppError } from '$lib/services/errors';
  import { playerStore } from '$lib/stores/player.svelte';
  import { downloadsStore } from '$lib/stores/downloads.svelte';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { notify } from '$lib/stores/toast';
  import SongRow from '$lib/components/music/SongRow.svelte';
  import SourceBadge, { sourceName } from '$lib/components/music/SourceBadge.svelte';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Checkbox } from '$lib/components/ui/checkbox/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import * as Empty from '$lib/components/ui/empty/index.js';
  import * as Pagination from '$lib/components/ui/pagination/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import * as Tabs from '$lib/components/ui/tabs/index.js';
  import SearchIcon from '@lucide/svelte/icons/search';
  import MusicIcon from '@lucide/svelte/icons/music';
  import LinkIcon from '@lucide/svelte/icons/link';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import SearchX from '@lucide/svelte/icons/search-x';
  import Download from '@lucide/svelte/icons/download';

  type SearchType = 'song' | 'playlist' | 'album';

  /** What the last completed search produced. Kept in one field so a new search can never mix shapes. */
  type Results =
    | { kind: 'none' }
    | { kind: 'songs'; songs: Song[]; total?: number }
    | { kind: 'collections'; collections: Collection[]; total?: number }
    | { kind: 'link'; source: SourceId; linkKind: CollectionKind; collection?: Collection; songs: Song[] };

  const songKey = (song: Song) => `${song.ref.source}:${song.ref.scope}:${song.ref.siteId}`;

  let query = $state('');
  let searchType = $state<SearchType>('song');
  let descriptors = $state<SourceDescriptor[]>([]);
  let selectedSources = $state<SourceId[]>([]);
  let page = $state(1);

  let results = $state<Results>({ kind: 'none' });
  let sourceErrors = $state<SourceFailure[]>([]);
  let respondedSources = $state(0);
  let queriedSources = $state(0);
  let searched = $state(false);
  let loading = $state(false);
  let failure = $state<string | null>(null);

  let selected = $state<Set<string>>(new Set());
  let batching = $state(false);
  /** Songs in the selection the library already has; -1 while unknown. */
  let alreadyOwned = $state(-1);

  let addToOpen = $state(false);
  let addToTargets = $state<Song[]>([]);
  let collections = $state<StoredCollection[]>([]);
  let collectionsLoading = $state(false);
  let collectionsError = $state<string | null>(null);

  const pageSize = $derived(settingsStore.settings?.pageSize ?? 20);

  /** Only sources that can serve the active tab may be queried. */
  const capableSources = $derived(
    descriptors
      .filter((descriptor) =>
        searchType === 'song'
          ? descriptor.capabilities.songSearch
          : searchType === 'playlist'
            ? descriptor.capabilities.playlistSearch
            : descriptor.capabilities.albumSearch
      )
      .map((descriptor) => descriptor.id)
  );

  const activeSources = $derived(selectedSources.filter((id) => capableSources.includes(id)));
  const isLink = $derived(query.trim().toLowerCase().startsWith('http'));
  const canSubmit = $derived(
    query.trim().length > 0 && !loading && (isLink || activeSources.length > 0)
  );

  const songs = $derived(
    results.kind === 'songs' ? results.songs : results.kind === 'link' ? results.songs : []
  );
  const total = $derived(
    results.kind === 'songs' || results.kind === 'collections' ? results.total : undefined
  );
  /** Without a backend total, keep the current page reachable and offer one more. */
  const pageCount = $derived(
    total ?? (results.kind === 'link' ? pageSize : page * pageSize + (songs.length || results.kind === 'collections' ? pageSize : 0))
  );
  const paginated = $derived(results.kind === 'songs' || results.kind === 'collections');

  const selectedSongs = $derived(songs.filter((song) => selected.has(songKey(song))));
  const allSelected = $derived(songs.length > 0 && selectedSongs.length === songs.length);
  const currentKey = $derived(playerStore.current ? songKey(playerStore.current) : null);

  /**
   * Asks the backend how much of the selection is already on disk, so the batch
   * button can warn before any task is created. Debounced because ticking
   * checkboxes would otherwise fire one scan per click.
   */
  $effect(() => {
    const picked = selectedSongs;
    if (picked.length === 0) {
      alreadyOwned = -1;
      return;
    }
    const timer = setTimeout(() => {
      void (async () => {
        try {
          const report = await localMusicService.precheck(
            picked.map((song) => ({
              name: song.title,
              artist: song.artists.map((artist) => artist.name).join(' / ')
            }))
          );
          alreadyOwned = report.skipped;
        } catch {
          // A failed precheck is only a missing hint; the batch still runs.
          alreadyOwned = -1;
        }
      })();
    }, 300);
    return () => clearTimeout(timer);
  });

  $effect(() => {
    if (!settingsStore.loaded && !settingsStore.loading) void settingsStore.load();
  });

  $effect(() => {
    if (descriptors.length > 0) return;
    void (async () => {
      try {
        const found = await searchService.getSourceCapabilities();
        descriptors = found;
        selectedSources = found.filter((d) => d.capabilities.songSearch).map((d) => d.id);
      } catch (cause) {
        failure = toAppError(cause).message;
      }
    })();
  });

  /** Default to every source able to serve the newly selected tab. */
  function switchType(next: string | undefined) {
    if (next !== 'song' && next !== 'playlist' && next !== 'album') return;
    if (next === searchType) return;
    searchType = next;
    page = 1;
    selected = new Set();
    results = { kind: 'none' };
    searched = false;
    sourceErrors = [];
    selectedSources = descriptors
      .filter((d) =>
        next === 'song'
          ? d.capabilities.songSearch
          : next === 'playlist'
            ? d.capabilities.playlistSearch
            : d.capabilities.albumSearch
      )
      .map((d) => d.id);
  }

  function toggleSource(source: SourceId) {
    selectedSources = selectedSources.includes(source)
      ? selectedSources.filter((item) => item !== source)
      : [...selectedSources, source];
    page = 1;
  }

  function onQueryInput() {
    page = 1;
  }

  async function submit(target = 1) {
    const trimmed = query.trim();
    if (!trimmed) return;
    page = target;
    loading = true;
    failure = null;
    selected = new Set();

    try {
      if (trimmed.toLowerCase().startsWith('http')) {
        const parsed = await searchService.parseShareLink(trimmed);
        results = {
          kind: 'link',
          source: parsed.source,
          linkKind: parsed.kind,
          collection: parsed.collection,
          songs: parsed.songs
        };
        sourceErrors = [];
        queriedSources = 1;
        respondedSources = 1;
      } else if (searchType === 'song') {
        const found = await searchService.searchSongs(trimmed, activeSources, target, pageSize);
        results = { kind: 'songs', songs: found.items.map((item) => item.song), total: found.total };
        sourceErrors = found.sourceErrors;
        queriedSources = found.queriedSources;
        respondedSources = found.respondedSources;
      } else {
        const found = await searchService.searchCollections(
          trimmed,
          searchType,
          activeSources,
          target,
          pageSize
        );
        results = { kind: 'collections', collections: found.items, total: found.total };
        sourceErrors = found.sourceErrors;
        queriedSources = found.queriedSources;
        respondedSources = found.respondedSources;
      }
      searched = true;
    } catch (cause) {
      const error = toAppError(cause);
      failure = error.message;
      results = { kind: 'none' };
      searched = true;
    } finally {
      loading = false;
    }
  }

  async function play(song: Song) {
    const index = songs.findIndex((item) => songKey(item) === songKey(song));
    if (index >= 0) await playerStore.setQueue(songs, index);
    await playerStore.playSong(song);
    if (playerStore.error) notify.fromError(playerStore.error);
  }

  async function download(song: Song) {
    await downloadsStore.enqueue(song);
    if (downloadsStore.error) notify.fromError(downloadsStore.error);
    else notify.success('已加入下载队列', song.title);
  }

  async function downloadSelected() {
    if (selectedSongs.length === 0) return;
    batching = true;
    const picked = selectedSongs;
    try {
      const report = await downloadService.createBatch(picked);
      await downloadsStore.load();
      notify.success(
        `已提交 ${picked.length} 首`,
        `成功 ${report.completed} · 跳过 ${report.skipped} · 失败 ${report.failed}`
      );
      selected = new Set();
    } catch (cause) {
      notify.fromError(toAppError(cause), '批量下载失败');
    } finally {
      batching = false;
    }
  }

  function toggleSelect(song: Song, checked: boolean) {
    const next = new Set(selected);
    if (checked) next.add(songKey(song));
    else next.delete(songKey(song));
    selected = next;
  }

  function toggleSelectAll(checked: boolean) {
    selected = checked ? new Set(songs.map(songKey)) : new Set();
  }

  async function openAddTo(songsToAdd: Song[]) {
    if (songsToAdd.length === 0) return;
    addToTargets = songsToAdd;
    addToOpen = true;
    collectionsLoading = true;
    collectionsError = null;
    try {
      collections = await collectionService.list();
    } catch (cause) {
      collectionsError = toAppError(cause).message;
    } finally {
      collectionsLoading = false;
    }
  }

  async function addTo(collection: StoredCollection) {
    try {
      const outcome: AddOutcome = await collectionService.addSongs(collection.id, addToTargets);
      notify.success(
        `已添加到「${collection.name}」`,
        `新增 ${outcome.added} · 跳过重复 ${outcome.skipped}`
      );
      addToOpen = false;
    } catch (cause) {
      notify.fromError(toAppError(cause), '添加到歌单失败');
    }
  }

  const typeLabels: Record<SearchType, string> = {
    song: '单曲',
    playlist: '歌单',
    album: '专辑'
  };
  const placeholders: Record<SearchType, string> = {
    song: '搜索歌曲、歌手，或粘贴分享链接',
    playlist: '搜索歌单，或粘贴歌单分享链接',
    album: '搜索专辑，或粘贴专辑分享链接'
  };
</script>

<svelte:head><title>搜索 - AutoOwl</title></svelte:head>

<div class="flex flex-col gap-6 p-6">
  <header class="flex flex-col gap-4">
    <h1 class="text-2xl font-semibold tracking-tight">搜索音乐</h1>

    <Tabs.Root value={searchType} onValueChange={switchType}>
      <Tabs.List>
        {#each ['song', 'playlist', 'album'] as const as value (value)}
          <Tabs.Trigger {value}>{typeLabels[value]}</Tabs.Trigger>
        {/each}
      </Tabs.List>
    </Tabs.Root>

    <form
      class="flex max-w-2xl items-center gap-2"
      onsubmit={(event) => {
        event.preventDefault();
        void submit(1);
      }}
    >
      <label class="sr-only" for="search-query">搜索关键词</label>
      <Input
        id="search-query"
        bind:value={query}
        oninput={onQueryInput}
        placeholder={placeholders[searchType]}
        autocomplete="off"
      />
      <Button type="submit" disabled={!canSubmit}>
        <SearchIcon />
        {loading ? '搜索中…' : '搜索'}
      </Button>
    </form>

    {#if isLink}
      <p class="flex items-center gap-1.5 text-sm text-muted-foreground">
        <LinkIcon class="size-3.5" />
        检测到分享链接，将直接解析该链接的内容
      </p>
    {/if}

    <fieldset class="flex flex-col gap-2" disabled={isLink}>
      <legend class="mb-1 text-sm font-medium text-muted-foreground">
        搜索源设置{#if !isLink}（已选 {activeSources.length} / {capableSources.length}）{/if}
      </legend>
      {#if descriptors.length === 0}
        <div class="flex gap-2">
          {#each Array(6) as _, i (i)}<Skeleton class="h-6 w-20" />{/each}
        </div>
      {:else}
        <div class="grid grid-cols-2 gap-x-4 gap-y-2 sm:grid-cols-3 lg:grid-cols-6">
          {#each capableSources as source (source)}
            <label class="flex items-center gap-2 text-sm" for={`source-${source}`}>
              <Checkbox
                id={`source-${source}`}
                checked={selectedSources.includes(source)}
                disabled={isLink}
                onCheckedChange={() => toggleSource(source)}
              />
              <span class="truncate">{sourceName(source)}</span>
            </label>
          {/each}
        </div>
      {/if}
    </fieldset>
  </header>

  {#if failure}
    <Alert.Root variant="destructive">
      <TriangleAlert />
      <Alert.Title>搜索失败</Alert.Title>
      <Alert.Description>{failure}</Alert.Description>
      <Alert.Action>
        <Button variant="outline" size="sm" onclick={() => void submit(page)}>重试</Button>
      </Alert.Action>
    </Alert.Root>
  {/if}

  {#if sourceErrors.length > 0}
    <Alert.Root>
      <TriangleAlert />
      <Alert.Title>
        部分来源未返回结果（{respondedSources} / {queriedSources} 个来源响应）
      </Alert.Title>
      <Alert.Description>
        <ul class="mt-1 flex flex-col gap-1">
          {#each sourceErrors as item (item.source + item.code)}
            <li class="flex items-center gap-2">
              <SourceBadge source={item.source} size="sm" />
              <span class="text-muted-foreground">{item.message}</span>
            </li>
          {/each}
        </ul>
      </Alert.Description>
    </Alert.Root>
  {/if}

  {#if results.kind === 'link' && results.collection}
    <Alert.Root>
      <LinkIcon />
      <Alert.Title>
        {results.linkKind === 'album' ? '专辑' : '歌单'}：{results.collection.name}
      </Alert.Title>
      <Alert.Description>
        <span class="flex items-center gap-2">
          <SourceBadge source={results.source} size="sm" />
          {results.collection.creator ?? ''}
          共 {results.songs.length} 首
        </span>
      </Alert.Description>
    </Alert.Root>
  {/if}

  {#if songs.length > 0}
    <div class="flex items-center gap-3">
      <span class="text-sm text-muted-foreground">已选 {selectedSongs.length} 首</span>
      {#if alreadyOwned > 0}
        <span class="text-sm text-amber-600 dark:text-amber-500 tabular-nums">
          其中 {alreadyOwned} 首已在本地音乐库，将被跳过
        </span>
      {/if}
      <Button
        variant="outline"
        size="sm"
        disabled={selectedSongs.length === 0 || batching}
        onclick={() => void downloadSelected()}
      >
        <Download />
        {batching ? '提交中…' : '下载选中'}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        disabled={selectedSongs.length === 0}
        onclick={() => void openAddTo(selectedSongs)}
      >
        添加选中到歌单
      </Button>
    </div>
  {/if}

  {#if loading}
    <div class="flex flex-col gap-2" aria-busy="true" aria-label="加载中">
      {#each Array(8) as _, i (i)}
        <div class="flex items-center gap-4">
          <Skeleton class="size-4" />
          <Skeleton class="h-4 w-6" />
          <Skeleton class="h-4 flex-1" />
          <Skeleton class="h-4 w-32" />
          <Skeleton class="h-4 w-10" />
        </div>
      {/each}
    </div>
  {:else if !searched}
    <Empty.Root class="border">
      <Empty.Header>
        <Empty.Media variant="icon"><SearchIcon /></Empty.Media>
        <Empty.Title>开始搜索</Empty.Title>
        <Empty.Description>
          输入歌曲、歌手或专辑名称，或直接粘贴分享链接自动解析。可在上方切换单曲、歌单、专辑，并选择搜索源。
        </Empty.Description>
      </Empty.Header>
    </Empty.Root>
  {:else if songs.length > 0}
    <Table.Root>
      <Table.Header>
        <Table.Row>
          <Table.Head class="w-10">
            <Checkbox
              checked={allSelected}
              indeterminate={selectedSongs.length > 0 && !allSelected}
              aria-label="全选本页歌曲"
              onCheckedChange={(checked) => toggleSelectAll(checked === true)}
            />
          </Table.Head>
          <Table.Head class="w-12 text-right">#</Table.Head>
          <Table.Head>标题</Table.Head>
          <Table.Head>歌手</Table.Head>
          <Table.Head>专辑</Table.Head>
          <Table.Head class="w-16 text-right">时长</Table.Head>
          <Table.Head class="w-32 text-right">操作</Table.Head>
        </Table.Row>
      </Table.Header>
      <Table.Body>
        {#each songs as song, index (songKey(song) + index)}
          <SongRow
            {song}
            {index}
            active={currentKey === songKey(song)}
            onplay={(value) => void play(value)}
            ondownload={(value) => void download(value)}
            onaddto={(value) => void openAddTo([value])}
          >
            {#snippet selectable()}
              <Checkbox
                checked={selected.has(songKey(song))}
                aria-label={`选择 ${song.title}`}
                onCheckedChange={(checked) => toggleSelect(song, checked === true)}
              />
            {/snippet}
          </SongRow>
        {/each}
      </Table.Body>
    </Table.Root>
  {:else if results.kind === 'collections' && results.collections.length > 0}
    <div class="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
      {#each results.collections as collection (collection.source + collection.id)}
        <a
          class="rounded-lg outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
          href={`${collection.contentType === 'album' ? '/album' : '/collections'}?source=${encodeURIComponent(collection.source)}&id=${encodeURIComponent(collection.id)}`}
        >
          <Card.Root class="h-full gap-3 py-4 transition-colors hover:bg-muted/50">
            <Card.Header class="px-4">
              <Card.Title class="line-clamp-2 text-base">{collection.name}</Card.Title>
              <Card.Description class="flex flex-wrap items-center gap-2">
                <SourceBadge source={collection.source} size="sm" />
                {#if collection.creator}<span class="truncate">{collection.creator}</span>{/if}
              </Card.Description>
            </Card.Header>
            <Card.Content class="px-4 text-sm text-muted-foreground">
              {collection.trackCount == null ? '' : `${collection.trackCount} 首`}
            </Card.Content>
          </Card.Root>
        </a>
      {/each}
    </div>
  {:else if !failure}
    <Empty.Root class="border">
      <Empty.Header>
        <Empty.Media variant="icon"><SearchX /></Empty.Media>
        <Empty.Title>没有找到结果</Empty.Title>
        <Empty.Description>
          换个关键词，或在上方勾选更多搜索源后重试。
        </Empty.Description>
      </Empty.Header>
    </Empty.Root>
  {/if}

  {#if paginated && searched && !failure && (songs.length > 0 || (results.kind === 'collections' && results.collections.length > 0))}
    <Pagination.Root count={pageCount} perPage={pageSize} {page} siblingCount={1}>
      {#snippet children({ pages, currentPage })}
        <Pagination.Content>
          <Pagination.Item>
            <Pagination.PrevButton
              disabled={currentPage <= 1 || loading}
              onclick={() => void submit(currentPage - 1)}
            />
          </Pagination.Item>
          {#each pages as item (item.key)}
            {#if item.type === 'ellipsis'}
              <Pagination.Item><Pagination.Ellipsis /></Pagination.Item>
            {:else}
              <Pagination.Item>
                <Pagination.Link
                  page={item}
                  isActive={currentPage === item.value}
                  onclick={() => void submit(item.value)}
                >
                  {item.value}
                </Pagination.Link>
              </Pagination.Item>
            {/if}
          {/each}
          <Pagination.Item>
            <Pagination.NextButton disabled={loading} onclick={() => void submit(currentPage + 1)} />
          </Pagination.Item>
        </Pagination.Content>
      {/snippet}
    </Pagination.Root>
  {/if}
</div>

<Dialog.Root bind:open={addToOpen}>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>添加到歌单</Dialog.Title>
      <Dialog.Description>
        选择一个本地歌单，将 {addToTargets.length} 首歌曲加入其中；重复的歌曲会自动跳过。
      </Dialog.Description>
    </Dialog.Header>

    {#if collectionsLoading}
      <div class="flex flex-col gap-2">
        {#each Array(4) as _, i (i)}<Skeleton class="h-10 w-full" />{/each}
      </div>
    {:else if collectionsError}
      <Alert.Root variant="destructive">
        <TriangleAlert />
        <Alert.Title>歌单列表加载失败</Alert.Title>
        <Alert.Description>{collectionsError}</Alert.Description>
        <Alert.Action>
          <Button variant="outline" size="sm" onclick={() => void openAddTo(addToTargets)}>
            重试
          </Button>
        </Alert.Action>
      </Alert.Root>
    {:else if collections.length === 0}
      <Empty.Root>
        <Empty.Header>
          <Empty.Media variant="icon"><MusicIcon /></Empty.Media>
          <Empty.Title>还没有本地歌单</Empty.Title>
          <Empty.Description>先到「我的歌单」新建一个歌单，再回来添加。</Empty.Description>
        </Empty.Header>
        <Empty.Content>
          <Button variant="outline" size="sm" href="/my-collections">前往我的歌单</Button>
        </Empty.Content>
      </Empty.Root>
    {:else}
      <ul class="flex max-h-72 flex-col gap-1 overflow-y-auto">
        {#each collections as collection (collection.id)}
          <li>
            <Button
              variant="ghost"
              class="h-auto w-full justify-start px-3 py-2 text-left"
              onclick={() => void addTo(collection)}
            >
              <span class="flex min-w-0 flex-1 items-center justify-between gap-2">
                <span class="truncate font-medium">{collection.name}</span>
                <span class="shrink-0 text-xs text-muted-foreground">
                  {collection.trackCount} 首
                </span>
              </span>
            </Button>
          </li>
        {/each}
      </ul>
    {/if}
  </Dialog.Content>
</Dialog.Root>
