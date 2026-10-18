<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import type {
    AppError,
    Collection,
    CollectionDetail,
    CollectionKind,
    Song,
    SourceId
  } from '$lib/domain/contracts';
  import { toAppError } from '$lib/services/errors';
  import { searchService } from '$lib/services/user/search-service';
  import { collectionService } from '$lib/services/user/collection-service';
  import { downloadsStore } from '$lib/stores/downloads.svelte';
  import { playerStore } from '$lib/stores/player.svelte';
  import { notify } from '$lib/stores/toast';
  import SongRow from '$lib/components/music/SongRow.svelte';
  import SourceBadge, { sourceName } from '$lib/components/music/SourceBadge.svelte';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Empty from '$lib/components/ui/empty/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Separator } from '$lib/components/ui/separator/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import ImportIcon from '@lucide/svelte/icons/import';
  import PlayIcon from '@lucide/svelte/icons/play';
  import SearchIcon from '@lucide/svelte/icons/search';
  import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';

  let {
    kind,
    basePath,
    title,
    searchPlaceholder
  }: {
    kind: CollectionKind;
    /** Route this component is mounted on, e.g. `/collections`. */
    basePath: string;
    title: string;
    searchPlaceholder: string;
  } = $props();

  const PAGE_SIZE = 30;
  /** Placeholder rows while a list loads; an array so `{#each}` can iterate it. */
  const SKELETON_ROWS = Array.from({ length: 8 }, (_, index) => index);
  // `kind` is a prop, so this must be derived rather than read once at init.
  const noun = $derived(kind === 'album' ? '专辑' : '歌单');

  /** `?source=` + `?id=` together switch the page from browse to detail. */
  const routeSource = $derived(page.url.searchParams.get('source') ?? '');
  const routeId = $derived(page.url.searchParams.get('id') ?? '');
  const routeQuery = $derived(page.url.searchParams.get('q') ?? '');
  const routePage = $derived(Math.max(1, Number(page.url.searchParams.get('page') ?? '1') || 1));
  const isDetail = $derived(routeSource !== '' && routeId !== '');

  // --- browse state -------------------------------------------------------
  let queryInput = $state('');
  let results = $state<Collection[]>([]);
  let resultTotal = $state<number | undefined>(undefined);
  let browseLoading = $state(false);
  let browseError = $state<AppError | null>(null);
  let browseSourceErrors = $state<string[]>([]);
  /** Distinguishes "not searched yet" from "searched, found nothing". */
  let browseSearched = $state(false);

  // --- detail state -------------------------------------------------------
  let detail = $state<CollectionDetail | null>(null);
  let detailLoading = $state(false);
  let detailError = $state<AppError | null>(null);
  let importing = $state(false);

  /**
   * Monotonic token per page instance. Every async load captures the token it
   * started with and writes state only while it is still the newest one, so a
   * slow response for an older `?id=` can never overwrite a newer one.
   */
  let requestToken = 0;
  /** Bumped by the retry buttons to re-run the effect with unchanged params. */
  let browseReloads = $state(0);
  let detailReloads = $state(0);

  const hasSongs = $derived((detail?.songs.length ?? 0) > 0);

  function trackCountLabel(collection: Collection): string {
    return collection.trackCount === undefined ? '曲目数未知' : `${collection.trackCount} 首`;
  }

  function detailHref(collection: Collection): string {
    const params = new URLSearchParams({ source: collection.source, id: collection.id });
    if (routeQuery) params.set('q', routeQuery);
    return `${basePath}?${params.toString()}`;
  }

  function browseHref(nextPage: number, query = routeQuery): string {
    const params = new URLSearchParams();
    if (query) params.set('q', query);
    if (nextPage > 1) params.set('page', String(nextPage));
    const suffix = params.toString();
    return suffix ? `${basePath}?${suffix}` : basePath;
  }

  function submitSearch(event: SubmitEvent): void {
    event.preventDefault();
    const next = queryInput.trim();
    if (!next) return;
    void goto(browseHref(1, next), { keepFocus: true, noScroll: true });
  }

  /** Runs on every `(kind, source, id, q, page)` change and on retry. */
  $effect(() => {
    const token = ++requestToken;
    const currentKind = kind;

    if (isDetail) {
      const source = routeSource;
      const id = routeId;
      void detailReloads;
      detailLoading = true;
      detailError = null;
      searchService
        .getCollectionDetail(source, currentKind, id)
        .then((loaded) => {
          if (token !== requestToken) return;
          detail = loaded;
          detailLoading = false;
        })
        .catch((cause: unknown) => {
          if (token !== requestToken) return;
          detail = null;
          detailError = toAppError(cause);
          detailLoading = false;
        });
      return;
    }

    // Browsing: a blank query has nothing to fetch, so show the prompt state.
    const query = routeQuery.trim();
    queryInput = routeQuery;
    detail = null;
    detailError = null;
    detailLoading = false;
    if (!query) {
      results = [];
      resultTotal = undefined;
      browseSourceErrors = [];
      browseSearched = false;
      browseLoading = false;
      browseError = null;
      return;
    }

    const pageNumber = routePage;
    void browseReloads;
    browseLoading = true;
    browseError = null;
    searchService
      .searchCollections(query, currentKind, [], pageNumber, PAGE_SIZE)
      .then((aggregated) => {
        if (token !== requestToken) return;
        results = aggregated.items;
        resultTotal = aggregated.total;
        browseSourceErrors = aggregated.sourceErrors.map(
          (failure) => `${sourceName(failure.source)}：${failure.message}`
        );
        browseSearched = true;
        browseLoading = false;
      })
      .catch((cause: unknown) => {
        if (token !== requestToken) return;
        results = [];
        resultTotal = undefined;
        browseSourceErrors = [];
        browseSearched = true;
        browseError = toAppError(cause);
        browseLoading = false;
      });
  });

  async function playAll(songs: Song[]): Promise<void> {
    if (songs.length === 0) return;
    await playerStore.setQueue(songs, 0);
    if (playerStore.error) {
      notify.fromError(playerStore.error, '播放失败');
      playerStore.clearError();
      return;
    }
    notify.success(`已开始播放 ${songs.length} 首歌曲`);
  }

  async function downloadAll(songs: Song[]): Promise<void> {
    if (songs.length === 0) return;
    await downloadsStore.enqueueBatch(songs);
    if (downloadsStore.error) {
      notify.fromError(downloadsStore.error, '批量下载失败');
      downloadsStore.clearError();
      return;
    }
    notify.success(`已加入下载队列：${songs.length} 首`);
  }

  async function playOne(song: Song): Promise<void> {
    await playerStore.playSong(song);
    if (playerStore.error) {
      notify.fromError(playerStore.error, '播放失败');
      playerStore.clearError();
    }
  }

  async function downloadOne(song: Song): Promise<void> {
    await downloadsStore.enqueue(song);
    if (downloadsStore.error) {
      notify.fromError(downloadsStore.error, '下载失败');
      downloadsStore.clearError();
      return;
    }
    notify.success(`已加入下载：${song.title}`);
  }

  async function importDetail(loaded: CollectionDetail): Promise<void> {
    importing = true;
    try {
      const stored = await collectionService.import(
        loaded.collection.name,
        loaded.songs,
        {
          source: loaded.collection.source,
          externalId: loaded.collection.id,
          description: loaded.collection.description,
          creator: loaded.collection.creator,
          coverUrl: loaded.collection.coverUrl
        },
        loaded.collection.contentType
      );
      notify.success(`已导入「${stored.name}」`, `共 ${stored.trackCount} 首，可在“我的歌单”查看`);
    } catch (cause) {
      notify.fromError(toAppError(cause), '导入失败');
    } finally {
      importing = false;
    }
  }

  const activeSongRef = $derived(playerStore.current?.ref);

  function isActive(song: Song): boolean {
    const current = activeSongRef;
    return (
      current !== undefined &&
      current.source === song.ref.source &&
      current.siteId === song.ref.siteId
    );
  }
</script>

<div class="flex flex-col gap-6 p-6">
  {#if isDetail}
    <div class="flex items-center gap-3">
      <Button variant="ghost" size="sm" href={browseHref(1)}>
        <ArrowLeftIcon class="size-4" />
        返回{noun}搜索
      </Button>
    </div>

    {#if detailLoading}
      <div class="flex flex-col gap-6">
        <div class="flex gap-6">
          <Skeleton class="size-40 rounded-lg" />
          <div class="flex flex-1 flex-col gap-3 py-2">
            <Skeleton class="h-7 w-64" />
            <Skeleton class="h-4 w-40" />
            <Skeleton class="h-4 w-full max-w-xl" />
            <Skeleton class="h-9 w-72" />
          </div>
        </div>
        <div class="flex flex-col gap-2">
          {#each SKELETON_ROWS as row (row)}
            <Skeleton class="h-12 w-full" />
          {/each}
        </div>
      </div>
    {:else if detailError}
      <Alert.Root variant="destructive">
        <TriangleAlertIcon class="size-4" />
        <Alert.Title>{noun}详情加载失败</Alert.Title>
        <Alert.Description>
          {detailError.message}（错误码：{detailError.code}）
        </Alert.Description>
        <Alert.Action>
          <Button variant="outline" size="sm" onclick={() => (detailReloads += 1)}>重试</Button>
        </Alert.Action>
      </Alert.Root>
    {:else if detail}
      {@const collection = detail.collection}
      <div class="flex flex-col gap-6 sm:flex-row">
        {#if collection.coverUrl}
          <img
            src={collection.coverUrl}
            alt={`${collection.name} 封面`}
            class="size-40 shrink-0 rounded-lg border border-border object-cover"
            loading="lazy"
          />
        {:else}
          <div
            class="flex size-40 shrink-0 items-center justify-center rounded-lg border border-border bg-muted text-muted-foreground"
          >
            <ImportIcon class="size-10" />
          </div>
        {/if}

        <div class="flex min-w-0 flex-1 flex-col gap-3">
          <div class="flex flex-wrap items-center gap-2">
            <h1 class="text-2xl font-semibold text-foreground">{collection.name}</h1>
            <SourceBadge source={collection.source} />
            <Badge variant="outline">{noun}</Badge>
          </div>
          <p class="text-sm text-muted-foreground">
            {collection.creator ? `${collection.creator} · ` : ''}共 {detail.songs.length} 首
            {#if collection.trackCount !== undefined && collection.trackCount !== detail.songs.length}
              （来源标记 {collection.trackCount} 首）
            {/if}
          </p>
          {#if collection.description}
            <p class="line-clamp-3 text-sm whitespace-pre-line text-muted-foreground">
              {collection.description}
            </p>
          {/if}
          <div class="flex flex-wrap gap-2 pt-1">
            <Button size="sm" disabled={!hasSongs} onclick={() => void playAll(detail!.songs)}>
              <PlayIcon class="size-4" />
              全部播放
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={!hasSongs}
              onclick={() => void downloadAll(detail!.songs)}
            >
              <DownloadIcon class="size-4" />
              全部下载
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={!hasSongs || importing}
              onclick={() => void importDetail(detail!)}
            >
              <ImportIcon class="size-4" />
              {importing ? '导入中…' : '导入为我的歌单'}
            </Button>
          </div>
        </div>
      </div>

      <Separator />

      {#if hasSongs}
        <div class="rounded-lg border border-border">
          <Table.Root>
            <Table.Header>
              <Table.Row>
                <Table.Head class="w-12 text-right">#</Table.Head>
                <Table.Head>标题</Table.Head>
                <Table.Head>歌手</Table.Head>
                <Table.Head>专辑</Table.Head>
                <Table.Head class="w-16 text-right">时长</Table.Head>
                <Table.Head class="w-32">操作</Table.Head>
              </Table.Row>
            </Table.Header>
            <Table.Body>
              {#each detail.songs as song, index (`${song.ref.source}:${song.ref.siteId}`)}
                <SongRow
                  {song}
                  {index}
                  active={isActive(song)}
                  onplay={(target) => void playOne(target)}
                  ondownload={(target) => void downloadOne(target)}
                />
              {/each}
            </Table.Body>
          </Table.Root>
        </div>
      {:else}
        <Empty.Root>
          <Empty.Header>
            <Empty.Media variant="icon"><SearchIcon /></Empty.Media>
            <Empty.Title>这个{noun}没有可用曲目</Empty.Title>
            <Empty.Description>
              来源可能限制了访问。试试在“账号 Cookie”里为 {sourceName(collection.source)} 补上登录信息，或换一个{noun}。
            </Empty.Description>
          </Empty.Header>
          <Empty.Content>
            <Button variant="outline" size="sm" href={browseHref(1)}>返回搜索</Button>
          </Empty.Content>
        </Empty.Root>
      {/if}
    {/if}
  {:else}
    <div class="flex flex-col gap-2">
      <h1 class="text-2xl font-semibold text-foreground">{title}</h1>
      <p class="text-sm text-muted-foreground">
        输入关键词搜索各平台的{noun}，点击卡片查看曲目并整体播放、下载或导入。
      </p>
    </div>

    <form class="flex max-w-xl gap-2" onsubmit={submitSearch}>
      <Input
        type="search"
        bind:value={queryInput}
        placeholder={searchPlaceholder}
        aria-label={`${noun}关键词`}
      />
      <Button type="submit" disabled={browseLoading || queryInput.trim() === ''}>
        <SearchIcon class="size-4" />
        搜索
      </Button>
    </form>

    {#if browseSourceErrors.length > 0}
      <Alert.Root>
        <TriangleAlertIcon class="size-4" />
        <Alert.Title>部分来源没有返回结果</Alert.Title>
        <Alert.Description>
          <ul class="list-disc pl-4">
            {#each browseSourceErrors as message (message)}
              <li>{message}</li>
            {/each}
          </ul>
        </Alert.Description>
      </Alert.Root>
    {/if}

    {#if browseLoading}
      <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
        {#each { length: 8 }}
          <div class="flex flex-col gap-3 rounded-lg border border-border p-4">
            <Skeleton class="aspect-square w-full rounded-md" />
            <Skeleton class="h-4 w-3/4" />
            <Skeleton class="h-3 w-1/2" />
          </div>
        {/each}
      </div>
    {:else if browseError}
      <Alert.Root variant="destructive">
        <TriangleAlertIcon class="size-4" />
        <Alert.Title>{noun}搜索失败</Alert.Title>
        <Alert.Description>
          {browseError.message}（错误码：{browseError.code}）
        </Alert.Description>
        <Alert.Action>
          <Button variant="outline" size="sm" onclick={() => (browseReloads += 1)}>重试</Button>
        </Alert.Action>
      </Alert.Root>
    {:else if !browseSearched}
      <Empty.Root>
        <Empty.Header>
          <Empty.Media variant="icon"><SearchIcon /></Empty.Media>
          <Empty.Title>还没有搜索{noun}</Empty.Title>
          <Empty.Description>
            在上面输入{noun}名称或关键词，例如“{searchPlaceholder}”，然后按回车。
          </Empty.Description>
        </Empty.Header>
      </Empty.Root>
    {:else if results.length === 0}
      <Empty.Root>
        <Empty.Header>
          <Empty.Media variant="icon"><SearchIcon /></Empty.Media>
          <Empty.Title>没有找到「{routeQuery}」相关的{noun}</Empty.Title>
          <Empty.Description>
            换一个更短的关键词，或先在“账号 Cookie”里补上平台登录信息再试。
          </Empty.Description>
        </Empty.Header>
      </Empty.Root>
    {:else}
      <p class="text-sm text-muted-foreground">
        「{routeQuery}」第 {routePage} 页，共 {results.length} 个{noun}{resultTotal !== undefined
          ? `（总计约 ${resultTotal} 个）`
          : ''}
      </p>
      <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
        {#each results as collection (`${collection.source}:${collection.id}`)}
          <Card.Root class="overflow-hidden p-0 transition-colors hover:border-primary">
            <a
              href={detailHref(collection)}
              class="flex h-full flex-col focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            >
              {#if collection.coverUrl}
                <img
                  src={collection.coverUrl}
                  alt={`${collection.name} 封面`}
                  class="aspect-square w-full object-cover"
                  loading="lazy"
                />
              {:else}
                <div
                  class="flex aspect-square w-full items-center justify-center bg-muted text-muted-foreground"
                >
                  <ImportIcon class="size-8" />
                </div>
              {/if}
              <div class="flex flex-1 flex-col gap-1 p-3">
                <span class="line-clamp-2 text-sm font-medium text-foreground">
                  {collection.name}
                </span>
                <span class="truncate text-xs text-muted-foreground">
                  {collection.creator || '未知创建者'}
                </span>
                <div class="mt-auto flex items-center justify-between pt-2">
                  <SourceBadge source={collection.source} size="sm" />
                  <span class="text-xs text-muted-foreground">{trackCountLabel(collection)}</span>
                </div>
              </div>
            </a>
          </Card.Root>
        {/each}
      </div>
      <div class="flex items-center gap-3">
        <Button
          variant="outline"
          size="sm"
          disabled={routePage <= 1}
          href={routePage <= 1 ? undefined : browseHref(routePage - 1)}
        >
          上一页
        </Button>
        <span class="text-sm text-muted-foreground">第 {routePage} 页</span>
        <Button
          variant="outline"
          size="sm"
          disabled={results.length < PAGE_SIZE}
          href={results.length < PAGE_SIZE ? undefined : browseHref(routePage + 1)}
        >
          下一页
        </Button>
      </div>
    {/if}
  {/if}
</div>
