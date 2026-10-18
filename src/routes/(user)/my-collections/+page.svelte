<script lang="ts">
  import type { AppError, Song, StoredCollection } from '$lib/domain/contracts';
  import { toAppError } from '$lib/services/errors';
  import { collectionService } from '$lib/services/user/collection-service';
  import { downloadsStore } from '$lib/stores/downloads.svelte';
  import { playerStore } from '$lib/stores/player.svelte';
  import { notify } from '$lib/stores/toast';
  import { formatDateTime } from '$lib/format';
  import SongRow from '$lib/components/music/SongRow.svelte';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import * as Empty from '$lib/components/ui/empty/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Separator } from '$lib/components/ui/separator/index.js';
  import { Skeleton } from '$lib/components/ui/skeleton/index.js';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import HeartIcon from '@lucide/svelte/icons/heart';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import PlayIcon from '@lucide/svelte/icons/play';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import TrashIcon from '@lucide/svelte/icons/trash';
  import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';

  let collections = $state<StoredCollection[]>([]);
  let listLoading = $state(true);
  let listError = $state<AppError | null>(null);

  /** Non-null when a collection is opened; drives the master/detail switch. */
  let opened = $state<StoredCollection | null>(null);
  let songs = $state<Song[]>([]);
  let songsLoading = $state(false);
  let songsError = $state<AppError | null>(null);

  let createOpen = $state(false);
  let createName = $state('');
  let createDescription = $state('');
  let creating = $state(false);

  let renameTarget = $state<StoredCollection | null>(null);
  let renameName = $state('');

  let deleteTarget = $state<StoredCollection | null>(null);

  const SKELETON_ROWS = Array.from({ length: 6 }, (_, index) => index);

  async function loadCollections(): Promise<void> {
    listLoading = true;
    listError = null;
    try {
      collections = await collectionService.list();
    } catch (error) {
      listError = toAppError(error);
    } finally {
      listLoading = false;
    }
  }

  async function openCollection(collection: StoredCollection): Promise<void> {
    opened = collection;
    songsLoading = true;
    songsError = null;
    songs = [];
    try {
      songs = await collectionService.getSongs(collection.id);
    } catch (error) {
      songsError = toAppError(error);
    } finally {
      songsLoading = false;
    }
  }

  /** Retry entry point: the template cannot narrow `opened` inside a closure. */
  async function reloadOpened(): Promise<void> {
    if (opened !== null) await openCollection(opened);
  }

  async function submitCreate(): Promise<void> {
    const name = createName.trim();
    if (name === '') return;
    creating = true;
    try {
      const created = await collectionService.create(
        name,
        createDescription.trim() === '' ? undefined : createDescription.trim()
      );
      // Prepend so the new collection is visible without a full reload.
      collections = [created, ...collections];
      createOpen = false;
      createName = '';
      createDescription = '';
      notify.success(`已创建歌单「${created.name}」`);
    } catch (error) {
      notify.error(toAppError(error).message);
    } finally {
      creating = false;
    }
  }

  async function submitRename(): Promise<void> {
    const target = renameTarget;
    const name = renameName.trim();
    if (target === null || name === '' || name === target.name) {
      renameTarget = null;
      return;
    }
    try {
      const updated = await collectionService.rename(target.id, name);
      collections = collections.map((item) => (item.id === updated.id ? updated : item));
      if (opened?.id === updated.id) opened = updated;
      notify.success('歌单已重命名');
    } catch (error) {
      notify.error(toAppError(error).message);
    } finally {
      renameTarget = null;
    }
  }

  async function confirmDelete(): Promise<void> {
    const target = deleteTarget;
    if (target === null) return;
    try {
      await collectionService.remove(target.id);
      collections = collections.filter((item) => item.id !== target.id);
      // Leaving the detail view open would show songs of a deleted collection.
      if (opened?.id === target.id) opened = null;
      notify.success(`已删除歌单「${target.name}」`);
    } catch (error) {
      notify.error(toAppError(error).message);
    } finally {
      deleteTarget = null;
    }
  }

  async function removeSong(song: Song): Promise<void> {
    const target = opened;
    if (target === null) return;
    try {
      await collectionService.removeSong(target.id, song.ref.siteId, song.ref.source);
      songs = songs.filter((item) => item.ref.siteId !== song.ref.siteId);
      const nextCount = Math.max(0, target.trackCount - 1);
      const updated = { ...target, trackCount: nextCount };
      opened = updated;
      collections = collections.map((item) => (item.id === updated.id ? updated : item));
    } catch (error) {
      notify.error(toAppError(error).message);
    }
  }

  async function playAll(): Promise<void> {
    if (songs.length === 0) return;
    await playerStore.setQueue(songs, 0);
    await playerStore.playSong(songs[0]);
  }

  async function downloadAll(): Promise<void> {
    if (songs.length === 0) return;
    try {
      await downloadsStore.enqueueBatch(songs);
      notify.success(`已加入下载队列：${songs.length} 首`);
    } catch (error) {
      notify.error(toAppError(error).message);
    }
  }

  $effect(() => {
    void loadCollections();
  });
</script>

<svelte:head><title>我的歌单 - AutoOwl</title></svelte:head>

<div class="flex flex-col gap-4 p-6">
  {#if opened === null}
    <div class="flex items-center justify-between gap-4">
      <div class="flex flex-col gap-1">
        <h1 class="text-xl font-semibold">我的歌单</h1>
        <p class="text-sm text-muted-foreground">本地保存的歌单，可手动创建或从搜索结果导入。</p>
      </div>
      <Button onclick={() => (createOpen = true)}>
        <PlusIcon class="size-4" />
        新建歌单
      </Button>
    </div>

    <Separator />

    {#if listLoading}
      <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {#each SKELETON_ROWS as row (row)}
          <Skeleton class="h-28 w-full rounded-lg" />
        {/each}
      </div>
    {:else if listError !== null}
      <Alert.Root variant="destructive">
        <TriangleAlertIcon class="size-4" />
        <Alert.Title>歌单加载失败</Alert.Title>
        <Alert.Description>{listError.message}（错误码：{listError.code}）</Alert.Description>
        <Alert.Action>
          <Button variant="outline" size="sm" onclick={() => void loadCollections()}>重试</Button>
        </Alert.Action>
      </Alert.Root>
    {:else if collections.length === 0}
      <Empty.Root class="border">
        <Empty.Header>
          <Empty.Media variant="icon"><HeartIcon /></Empty.Media>
          <Empty.Title>还没有歌单</Empty.Title>
          <Empty.Description>新建一个歌单，或在搜索结果里把歌曲添加进来。</Empty.Description>
        </Empty.Header>
        <Empty.Content>
          <Button onclick={() => (createOpen = true)}>
            <PlusIcon class="size-4" />
            新建歌单
          </Button>
        </Empty.Content>
      </Empty.Root>
    {:else}
      <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {#each collections as collection (collection.id)}
          <Card.Root class="flex flex-col">
            <Card.Header class="min-w-0">
              <Card.Title class="flex min-w-0 items-center gap-2">
                <span class="truncate" title={collection.name}>{collection.name}</span>
                {#if collection.kind === 'imported'}
                  <Badge variant="secondary" class="shrink-0">导入</Badge>
                {/if}
              </Card.Title>
              <Card.Description class="truncate">
                {collection.description ?? `${collection.trackCount} 首 · 更新于 ${formatDateTime(collection.updatedAt)}`}
              </Card.Description>
            </Card.Header>
            <Card.Footer class="mt-auto flex items-center justify-between gap-2">
              <Button variant="secondary" size="sm" onclick={() => void openCollection(collection)}>
                查看（{collection.trackCount}）
              </Button>
              <div class="flex items-center gap-1">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label={`重命名 ${collection.name}`}
                  onclick={() => {
                    renameTarget = collection;
                    renameName = collection.name;
                  }}
                >
                  <PencilIcon class="size-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label={`删除 ${collection.name}`}
                  onclick={() => (deleteTarget = collection)}
                >
                  <TrashIcon class="size-4" />
                </Button>
              </div>
            </Card.Footer>
          </Card.Root>
        {/each}
      </div>
    {/if}
  {:else}
    <div class="flex items-center gap-3">
      <Button variant="ghost" size="sm" onclick={() => (opened = null)}>
        <ArrowLeftIcon class="size-4" />
        返回
      </Button>
      <h1 class="min-w-0 truncate text-xl font-semibold">{opened.name}</h1>
      <Badge variant="secondary">{opened.trackCount} 首</Badge>
      <div class="ml-auto flex items-center gap-2">
        <Button size="sm" disabled={songs.length === 0} onclick={() => void playAll()}>
          <PlayIcon class="size-4" />
          播放全部
        </Button>
        <Button
          variant="secondary"
          size="sm"
          disabled={songs.length === 0}
          onclick={() => void downloadAll()}
        >
          <DownloadIcon class="size-4" />
          下载全部
        </Button>
      </div>
    </div>

    <Separator />

    {#if songsLoading}
      <div class="flex flex-col gap-2">
        {#each SKELETON_ROWS as row (row)}
          <Skeleton class="h-12 w-full" />
        {/each}
      </div>
    {:else if songsError !== null}
      <Alert.Root variant="destructive">
        <TriangleAlertIcon class="size-4" />
        <Alert.Title>歌曲加载失败</Alert.Title>
        <Alert.Description>{songsError.message}（错误码：{songsError.code}）</Alert.Description>
        <Alert.Action>
          <Button variant="outline" size="sm" onclick={() => void reloadOpened()}>
            重试
          </Button>
        </Alert.Action>
      </Alert.Root>
    {:else if songs.length === 0}
      <Empty.Root class="border">
        <Empty.Header>
          <Empty.Media variant="icon"><HeartIcon /></Empty.Media>
          <Empty.Title>歌单是空的</Empty.Title>
          <Empty.Description>在搜索结果里用「添加到歌单」把歌曲放进来。</Empty.Description>
        </Empty.Header>
        <Empty.Content>
          <Button variant="outline" size="sm" href="/search">前往搜索</Button>
        </Empty.Content>
      </Empty.Root>
    {:else}
      <Table.Root>
        <Table.Header>
          <Table.Row>
            <Table.Head class="w-12 text-right">#</Table.Head>
            <Table.Head>歌曲</Table.Head>
            <Table.Head>歌手</Table.Head>
            <Table.Head>专辑</Table.Head>
            <Table.Head class="w-16 text-right">时长</Table.Head>
            <Table.Head class="w-32 text-right">操作</Table.Head>
          </Table.Row>
        </Table.Header>
        <Table.Body>
          {#each songs as song, index (`${song.ref.source}/${song.ref.siteId}`)}
            <SongRow
              {song}
              {index}
              active={playerStore.current?.ref.siteId === song.ref.siteId &&
                playerStore.current?.ref.source === song.ref.source}
              onplay={(target) => void playerStore.playSong(target)}
              ondownload={(target) => void downloadsStore.enqueue(target)}
              onaddto={(target) => void removeSong(target)}
            />
          {/each}
        </Table.Body>
      </Table.Root>
    {/if}
  {/if}
</div>

<Dialog.Root bind:open={createOpen}>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>新建歌单</Dialog.Title>
      <Dialog.Description>歌单保存在本地，不会上传到任何平台。</Dialog.Description>
    </Dialog.Header>
    <div class="flex flex-col gap-3">
      <Input placeholder="歌单名称" bind:value={createName} aria-label="歌单名称" />
      <Input placeholder="描述（可选）" bind:value={createDescription} aria-label="歌单描述" />
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (createOpen = false)}>取消</Button>
      <Button disabled={creating || createName.trim() === ''} onclick={() => void submitCreate()}>
        {creating ? '创建中…' : '创建'}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root
  open={renameTarget !== null}
  onOpenChange={(open) => {
    if (!open) renameTarget = null;
  }}
>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>重命名歌单</Dialog.Title>
    </Dialog.Header>
    <Input bind:value={renameName} aria-label="新的歌单名称" />
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (renameTarget = null)}>取消</Button>
      <Button disabled={renameName.trim() === ''} onclick={() => void submitRename()}>保存</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root
  open={deleteTarget !== null}
  onOpenChange={(open) => {
    if (!open) deleteTarget = null;
  }}
>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>删除歌单</Dialog.Title>
      <Dialog.Description>
        确定删除「{deleteTarget?.name}」吗？歌单内的歌曲记录会一并移除，已下载的文件不受影响。
      </Dialog.Description>
    </Dialog.Header>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (deleteTarget = null)}>取消</Button>
      <Button variant="destructive" onclick={() => void confirmDelete()}>删除</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
