import type {
  AddOutcome,
  CollectionType,
  ImportMetadata,
  Song,
  SongKey,
  SourceId,
  StoredCollection
} from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

/** Locally stored collections: user-made playlists and imported listings. */
export const collectionService = {
  list(): Promise<StoredCollection[]> {
    return client.invoke<StoredCollection[]>('list_collections');
  },

  create(name: string, description?: string): Promise<StoredCollection> {
    return client.invoke<StoredCollection>('create_collection', { name, description });
  },

  rename(id: string, name: string): Promise<StoredCollection> {
    return client.invoke<StoredCollection>('rename_collection', { id, name });
  },

  remove(id: string): Promise<void> {
    return client.invoke<void>('delete_collection', { id });
  },

  getSongs(id: string): Promise<Song[]> {
    return client.invoke<Song[]>('get_collection_songs', { id });
  },

  /** Duplicates are skipped and counted in the outcome, not rejected. */
  addSongs(id: string, songs: Song[]): Promise<AddOutcome> {
    return client.invoke<AddOutcome>('add_collection_songs', { id, songs });
  },

  removeSong(id: string, songId: string, source: SourceId): Promise<void> {
    return client.invoke<void>('remove_collection_song', { id, songId, source });
  },

  /** `order` is the complete target ordering, as `(songId, source)` pairs. */
  reorderSongs(id: string, order: SongKey[]): Promise<void> {
    return client.invoke<void>('reorder_collection_songs', { id, order });
  },

  /** Persists a remote listing (share link or search result) as a read-only collection. */
  import(
    name: string,
    songs: Song[],
    metadata?: ImportMetadata,
    contentType?: CollectionType
  ): Promise<StoredCollection> {
    return client.invoke<StoredCollection>('import_collection', {
      name,
      songs,
      metadata,
      contentType
    });
  }
};
