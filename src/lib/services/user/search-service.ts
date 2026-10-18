import type {
  AggregatedCollections,
  AggregatedSearch,
  CollectionDetail,
  CollectionKind,
  CollectionQuery,
  ParsedShareLink,
  SearchQuery,
  SourceDescriptor,
  SourceId
} from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

/** Remote search: songs, playlists/albums, share links and source capabilities. */
export const searchService = {
  /** Server-side clamp/trim of a song query before it is issued. */
  validateSearch(request: SearchQuery): Promise<SearchQuery> {
    return client.invoke<SearchQuery>('validate_search', { request });
  },

  /** Server-side clamp/trim of a collection query before it is issued. */
  validateCollection(request: CollectionQuery): Promise<CollectionQuery> {
    return client.invoke<CollectionQuery>('validate_collection', { request });
  },

  searchSongs(
    query: string,
    sources: SourceId[],
    page: number,
    pageSize: number
  ): Promise<AggregatedSearch> {
    return client.invoke<AggregatedSearch>('search_songs', { query, sources, page, pageSize });
  },

  searchCollections(
    query: string,
    kind: CollectionKind,
    sources: SourceId[],
    page: number,
    pageSize: number
  ): Promise<AggregatedCollections> {
    return client.invoke<AggregatedCollections>('search_collections', {
      query,
      kind,
      sources,
      page,
      pageSize
    });
  },

  getCollectionDetail(
    source: SourceId,
    kind: CollectionKind,
    id: string
  ): Promise<CollectionDetail> {
    return client.invoke<CollectionDetail>('get_collection_detail', { source, kind, id });
  },

  parseShareLink(url: string): Promise<ParsedShareLink> {
    return client.invoke<ParsedShareLink>('parse_share_link', { url });
  },

  getSourceCapabilities(): Promise<SourceDescriptor[]> {
    return client.invoke<SourceDescriptor[]>('get_source_capabilities');
  }
};
