<script lang="ts">
  import CollectionExplorer from '$lib/components/music/CollectionExplorer.svelte';
</script>

<svelte:head><title>歌单 - AutoOwl</title></svelte:head>

<CollectionExplorer
  kind="playlist"
  basePath="/collections"
  title="歌单"
  searchPlaceholder="搜索歌单名称，例如：华语流行"
/>
