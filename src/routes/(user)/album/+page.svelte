<script lang="ts">
  import CollectionExplorer from '$lib/components/music/CollectionExplorer.svelte';
</script>

<svelte:head><title>专辑 - AutoOwl</title></svelte:head>

<CollectionExplorer
  kind="album"
  basePath="/album"
  title="专辑"
  searchPlaceholder="搜索专辑名称，例如：范特西"
/>
