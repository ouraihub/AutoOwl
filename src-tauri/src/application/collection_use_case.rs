//! "My collections" use cases.
//!
//! Wraps `CollectionRepository` with the validation the interface layer would
//! otherwise repeat, and converts an external playlist into an import.

use std::sync::Arc;

use crate::domain::collection::{Collection, CollectionType};
use crate::domain::source::{CollectionDetail, Song};
use crate::infrastructure::collection_repository::{
    AddOutcome, CollectionRepository, CollectionStoreError, ImportMetadata, StoredCollection,
};
use crate::infrastructure::storage::Database;

pub struct CollectionService {
    repository: CollectionRepository,
}

impl CollectionService {
    pub fn new(database: Arc<Database>) -> Self {
        Self {
            repository: CollectionRepository::new(database),
        }
    }

    pub fn list(&self) -> Result<Vec<StoredCollection>, CollectionStoreError> {
        self.repository.list()
    }

    pub fn create(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<StoredCollection, CollectionStoreError> {
        validate_collection_name(name)?;
        self.repository.create(name, description)
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<StoredCollection, CollectionStoreError> {
        validate_collection_name(name)?;
        self.repository.rename(id, name)
    }

    pub fn delete(&self, id: &str) -> Result<(), CollectionStoreError> {
        self.repository.delete(id)
    }

    pub fn songs(&self, id: &str) -> Result<Vec<Song>, CollectionStoreError> {
        self.repository.songs(id)
    }

    /// Adds songs, tolerating ones already saved.
    pub fn add_songs(&self, id: &str, songs: &[Song]) -> Result<AddOutcome, CollectionStoreError> {
        self.repository.add_songs(id, songs)
    }

    /// Adds exactly one song, reporting a duplicate as an error.
    pub fn add_song(&self, id: &str, song: &Song) -> Result<(), CollectionStoreError> {
        self.repository.add_song(id, song)
    }

    pub fn remove_song(
        &self,
        id: &str,
        song_id: &str,
        source: &str,
    ) -> Result<(), CollectionStoreError> {
        self.repository.remove_song(id, song_id, source)
    }

    pub fn reorder(
        &self,
        id: &str,
        ordered_keys: &[(String, String)],
    ) -> Result<(), CollectionStoreError> {
        self.repository.reorder(id, ordered_keys)
    }

    /// Imports an external playlist, atomically.
    pub fn import(
        &self,
        name: &str,
        songs: &[Song],
        metadata: &ImportMetadata,
        content_type: CollectionType,
    ) -> Result<StoredCollection, CollectionStoreError> {
        validate_collection_name(name)?;
        self.repository.import(name, songs, metadata, content_type)
    }

    /// Imports a `CollectionDetail` as fetched from a source.
    ///
    /// The source's own name, creator and cover are carried over so the
    /// imported collection looks like the source playlist in the list view.
    pub fn import_detail(
        &self,
        detail: &CollectionDetail,
    ) -> Result<StoredCollection, CollectionStoreError> {
        let collection = &detail.collection;
        self.import(
            &collection.name,
            &detail.songs,
            &import_metadata_from(collection),
            collection.content_type,
        )
    }
}

/// Import metadata carried over from a fetched collection.
pub fn import_metadata_from(collection: &Collection) -> ImportMetadata {
    ImportMetadata {
        source: Some(collection.source.clone()),
        external_id: Some(collection.id.clone()),
        description: collection.description.clone(),
        creator: collection.creator.clone(),
        cover_url: collection.cover_url.clone(),
        // `original_link` yields an empty string for sources with no canonical
        // web URL; store that as absent rather than as a blank link.
        link: Some(crate::domain::link::original_link(
            &collection.source,
            &collection.id,
            match collection.content_type {
                CollectionType::Album => crate::domain::link::LinkKind::Album,
                CollectionType::Playlist => crate::domain::link::LinkKind::Playlist,
            },
        ))
        .filter(|link| !link.is_empty()),
    }
}

pub fn validate_collection_name(name: &str) -> Result<(), CollectionStoreError> {
    if name.trim().is_empty() || name.chars().count() > 200 {
        return Err(CollectionStoreError::InvalidArgument(
            "歌单名称无效".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{import_metadata_from, validate_collection_name, CollectionService};
    use crate::domain::collection::{Collection, CollectionType};
    use crate::domain::source::{CollectionDetail, Song, SongRef};
    use crate::infrastructure::storage::Database;

    fn service() -> CollectionService {
        let database = Arc::new(Database::open_in_memory().expect("open"));
        CollectionService::new(database)
    }

    fn song(site_id: &str, title: &str) -> Song {
        Song {
            reference: SongRef {
                source: "netease".into(),
                scope: "song".into(),
                site_id: site_id.into(),
            },
            title: title.into(),
            artists: Vec::new(),
            album: None,
            duration_ms: None,
            cover_url: None,
        }
    }

    #[test]
    fn name_validation_matches_the_repository_bound() {
        assert!(validate_collection_name("歌单").is_ok());
        assert!(validate_collection_name("   ").is_err());
        assert!(validate_collection_name(&"字".repeat(201)).is_err());
    }

    #[test]
    fn create_then_add_then_list_reflects_the_track_count() {
        let service = service();
        let created = service.create("歌单", None).expect("create");
        service
            .add_songs(&created.id, &[song("1", "A"), song("2", "B")])
            .expect("add");

        let listed = service.list().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].track_count, 2);
        assert_eq!(service.songs(&created.id).expect("songs").len(), 2);
    }

    #[test]
    fn importing_a_fetched_detail_carries_the_source_identity() {
        let detail = CollectionDetail {
            collection: Collection {
                id: "9527".into(),
                source: "netease".into(),
                content_type: CollectionType::Playlist,
                name: "外部歌单".into(),
                description: Some("说明".into()),
                creator: Some("某人".into()),
                cover_url: Some("http://example.com/c.jpg".into()),
                track_count: Some(2),
            },
            songs: vec![song("1", "A"), song("2", "B")],
        };

        let service = service();
        let imported = service.import_detail(&detail).expect("import");
        assert_eq!(imported.name, "外部歌单");
        assert_eq!(imported.source, "netease");
        assert_eq!(imported.external_id.as_deref(), Some("9527"));
        assert_eq!(imported.creator.as_deref(), Some("某人"));
        assert_eq!(imported.track_count, 2);
        assert!(imported.kind.is_imported());

        // Imported collections are read-only for adds.
        assert!(service.add_song(&imported.id, &song("3", "C")).is_err());
    }

    #[test]
    fn import_metadata_includes_the_original_link_when_the_source_has_one() {
        let collection = Collection {
            id: "9527".into(),
            source: "netease".into(),
            content_type: CollectionType::Playlist,
            name: "歌单".into(),
            description: None,
            creator: None,
            cover_url: None,
            track_count: None,
        };
        let metadata = import_metadata_from(&collection);
        assert_eq!(metadata.source.as_deref(), Some("netease"));
        assert_eq!(metadata.external_id.as_deref(), Some("9527"));
        assert!(metadata
            .link
            .as_deref()
            .map(|link| link.contains("9527"))
            .unwrap_or(false));
    }
}
