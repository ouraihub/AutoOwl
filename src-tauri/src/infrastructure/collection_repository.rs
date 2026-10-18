//! "My collections" backed by `collections` + `collection_songs`.
//!
//! Local playlists: user-created collections plus
//! collections imported wholesale from an external playlist. The composite
//! primary key `(collection_id, song_id, source)` is what makes a song unique
//! within a collection, so duplicate adds are rejected by the schema rather
//! than by a read-then-write race.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::domain::clock::{now_iso8601, unix_millis};
use crate::domain::collection::CollectionType;
use crate::domain::source::Song;
use crate::infrastructure::storage::{Database, StorageError};

/// Longest accepted collection name.
const MAX_NAME_LENGTH: usize = 200;

/// Cap on one import, so a hostile payload cannot wedge the transaction.
const MAX_IMPORT_SONGS: usize = 20_000;

#[derive(Debug, thiserror::Error)]
pub enum CollectionStoreError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("collection not found: {0}")]
    NotFound(String),
    #[error("song already in collection: {0}")]
    DuplicateSong(String),
    #[error("collection is imported and cannot be edited: {0}")]
    ImportedReadOnly(String),
}

impl CollectionStoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Storage(_) => "STORAGE_ERROR",
            Self::InvalidArgument(_) => "INVALID_ARGUMENT",
            Self::NotFound(_) => "COLLECTION_NOT_FOUND",
            Self::DuplicateSong(_) => "COLLECTION_SONG_DUPLICATE",
            Self::ImportedReadOnly(_) => "COLLECTION_IMPORTED_READONLY",
        }
    }

    /// User-facing message. The `Display` form carries an English prefix for
    /// logs; the UI gets Chinese only.
    pub fn message(&self) -> String {
        match self {
            Self::Storage(_) => "数据库操作失败".to_owned(),
            Self::InvalidArgument(detail) => detail.clone(),
            Self::NotFound(_) => "歌单或曲目不存在".to_owned(),
            Self::DuplicateSong(_) => "该歌曲已在歌单中".to_owned(),
            Self::ImportedReadOnly(_) => "导入的歌单不能添加曲目".to_owned(),
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::Storage(_))
    }
}

/// How a collection came to exist.
///
/// Imported collections are snapshots of somebody else's playlist, so the UI
/// forbids adding tracks to them; user collections are freely editable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CollectionKind {
    User,
    Imported,
}

impl CollectionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Imported => "imported",
        }
    }

    fn parse(raw: &str) -> Self {
        // Unknown values are treated as user collections: an editable
        // collection is the safe default for a row we cannot classify.
        if raw == "imported" {
            Self::Imported
        } else {
            Self::User
        }
    }

    pub fn is_imported(self) -> bool {
        matches!(self, Self::Imported)
    }
}

/// A stored collection plus its track count, as the list view renders it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCollection {
    pub id: String,
    pub kind: CollectionKind,
    pub content_type: CollectionType,
    pub source: String,
    pub external_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub creator: Option<String>,
    pub cover_url: Option<String>,
    pub link: Option<String>,
    pub track_count: u64,
    pub created_at: String,
    pub updated_at: String,
}

/// Metadata for an imported external playlist.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportMetadata {
    pub source: Option<String>,
    pub external_id: Option<String>,
    pub description: Option<String>,
    pub creator: Option<String>,
    pub cover_url: Option<String>,
    pub link: Option<String>,
}

/// Outcome of a batch add; `skipped` counts songs already present.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddOutcome {
    pub requested: u64,
    pub added: u64,
    pub skipped: u64,
}

pub struct CollectionRepository {
    database: Arc<Database>,
}

impl CollectionRepository {
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    /// Creates a user collection.
    pub fn create(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<StoredCollection, CollectionStoreError> {
        let name = validate_name(name)?;
        let id = new_id("col");
        let timestamp = now_iso8601();
        self.database.with(|connection| {
            insert_collection(
                connection,
                &id,
                CollectionKind::User,
                &name,
                &ImportMetadata {
                    description: description.map(str::to_owned),
                    ..Default::default()
                },
                &timestamp,
            )
        })?;
        self.get(&id)?
            .ok_or_else(|| CollectionStoreError::NotFound(id))
    }

    /// Renames a collection; imported collections may also be renamed because
    /// the name is local labelling, not imported content.
    pub fn rename(&self, id: &str, name: &str) -> Result<StoredCollection, CollectionStoreError> {
        let name = validate_name(name)?;
        let updated = self.database.with(|connection| {
            connection.execute(
                "UPDATE collections SET name = ?1, updated_at = ?2 WHERE id = ?3",
                params![name, now_iso8601(), id],
            )
        })?;
        if updated == 0 {
            return Err(CollectionStoreError::NotFound(id.to_owned()));
        }
        self.get(id)?
            .ok_or_else(|| CollectionStoreError::NotFound(id.to_owned()))
    }

    /// Deletes a collection and, by `ON DELETE CASCADE`, its songs.
    pub fn delete(&self, id: &str) -> Result<(), CollectionStoreError> {
        let removed = self.database.with(|connection| {
            connection.execute("DELETE FROM collections WHERE id = ?1", params![id])
        })?;
        if removed == 0 {
            return Err(CollectionStoreError::NotFound(id.to_owned()));
        }
        Ok(())
    }

    /// All collections, newest first, each with its track count.
    pub fn list(&self) -> Result<Vec<StoredCollection>, CollectionStoreError> {
        Ok(self.database.with(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.id, c.kind, c.content_type, c.source, c.external_id, c.name, \
                        c.description, c.creator, c.cover_url, c.link, c.created_at, c.updated_at, \
                        (SELECT COUNT(*) FROM collection_songs s WHERE s.collection_id = c.id) \
                 FROM collections c \
                 ORDER BY c.created_at DESC, c.id DESC",
            )?;
            let rows = statement.query_map([], read_collection)?;
            rows.collect::<Result<Vec<_>, _>>()
        })?)
    }

    /// Single collection by id.
    pub fn get(&self, id: &str) -> Result<Option<StoredCollection>, CollectionStoreError> {
        Ok(self.database.with(|connection| {
            connection
                .query_row(
                    "SELECT c.id, c.kind, c.content_type, c.source, c.external_id, c.name, \
                            c.description, c.creator, c.cover_url, c.link, c.created_at, \
                            c.updated_at, \
                            (SELECT COUNT(*) FROM collection_songs s WHERE s.collection_id = c.id) \
                     FROM collections c WHERE c.id = ?1",
                    params![id],
                    read_collection,
                )
                .optional()
        })?)
    }

    /// Adds one song. Rejects a song already in the collection so the UI can
    /// tell the user instead of silently doing nothing.
    pub fn add_song(&self, id: &str, song: &Song) -> Result<(), CollectionStoreError> {
        let outcome = self.add_songs(id, std::slice::from_ref(song))?;
        if outcome.added == 0 {
            return Err(CollectionStoreError::DuplicateSong(
                song.reference.site_id.clone(),
            ));
        }
        Ok(())
    }

    /// Adds many songs, skipping ones already present.
    ///
    /// A batch add is idempotent by design: the "add all results" button must
    /// not fail because one track was already saved.
    pub fn add_songs(&self, id: &str, songs: &[Song]) -> Result<AddOutcome, CollectionStoreError> {
        self.add_songs_internal(id, songs)
    }

    /// Appends songs already absent from the collection, in one transaction.
    fn add_songs_internal(
        &self,
        id: &str,
        songs: &[Song],
    ) -> Result<AddOutcome, CollectionStoreError> {
        let collection = self
            .get(id)?
            .ok_or_else(|| CollectionStoreError::NotFound(id.to_owned()))?;
        if collection.kind.is_imported() {
            return Err(CollectionStoreError::ImportedReadOnly(id.to_owned()));
        }
        if songs.is_empty() {
            return Err(CollectionStoreError::InvalidArgument(
                "没有可添加的歌曲".to_owned(),
            ));
        }
        for song in songs {
            validate_song(song)?;
        }

        let timestamp = now_iso8601();
        let added = self.database.with_transaction(|transaction| {
            let mut position = next_position(transaction, id)?;
            let mut added = 0_u64;
            for song in songs {
                if insert_song(transaction, id, song, position, &timestamp)? {
                    added = added.saturating_add(1);
                    position = position.saturating_add(1);
                }
            }
            // Only a real change bumps `updated_at`, so the list view's sort
            // does not jump for a no-op add.
            if added > 0 {
                touch(transaction, id, &timestamp)?;
            }
            Ok(added)
        })?;

        let requested = songs.len() as u64;
        Ok(AddOutcome {
            requested,
            added,
            skipped: requested.saturating_sub(added),
        })
    }

    /// Removes one song from a collection.
    pub fn remove_song(
        &self,
        id: &str,
        song_id: &str,
        source: &str,
    ) -> Result<(), CollectionStoreError> {
        let removed = self.database.with_transaction(|transaction| {
            let removed = transaction.execute(
                "DELETE FROM collection_songs \
                 WHERE collection_id = ?1 AND song_id = ?2 AND source = ?3",
                params![id, song_id, source],
            )?;
            if removed > 0 {
                touch(transaction, id, &now_iso8601())?;
            }
            Ok(removed)
        })?;
        if removed == 0 {
            return Err(CollectionStoreError::NotFound(format!("{id}/{song_id}")));
        }
        Ok(())
    }

    /// Songs in stored order.
    pub fn songs(&self, id: &str) -> Result<Vec<Song>, CollectionStoreError> {
        if self.get(id)?.is_none() {
            return Err(CollectionStoreError::NotFound(id.to_owned()));
        }
        let payloads = self.database.with(|connection| {
            let mut statement = connection.prepare(
                "SELECT song_json FROM collection_songs \
                 WHERE collection_id = ?1 ORDER BY position ASC, song_id ASC",
            )?;
            let rows = statement.query_map(params![id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })?;
        Ok(payloads
            .iter()
            .filter_map(|payload| match serde_json::from_str::<Song>(payload) {
                Ok(song) => Some(song),
                Err(error) => {
                    tracing::warn!(event = "collection_song_unreadable", collection = id, %error);
                    None
                }
            })
            .collect())
    }

    /// Rewrites positions to match `ordered_keys` (`(song_id, source)` pairs).
    ///
    /// Keys not listed keep their relative order after the listed ones, so a
    /// partial reorder from a drag-and-drop UI cannot lose tracks.
    pub fn reorder(
        &self,
        id: &str,
        ordered_keys: &[(String, String)],
    ) -> Result<(), CollectionStoreError> {
        if self.get(id)?.is_none() {
            return Err(CollectionStoreError::NotFound(id.to_owned()));
        }
        let timestamp = now_iso8601();
        self.database.with_transaction(|transaction| {
            let current = current_keys(transaction, id)?;
            // Build the target order first, then write dense positions 0..n.
            // Shifting existing positions in place would let an unlisted row
            // collide with a row that was just moved.
            let mut target: Vec<(String, String)> = Vec::with_capacity(current.len());
            for key in ordered_keys {
                if current.contains(key) && !target.contains(key) {
                    target.push(key.clone());
                }
            }
            for key in current {
                if !target.contains(&key) {
                    target.push(key);
                }
            }
            for (position, (song_id, source)) in target.iter().enumerate() {
                transaction.execute(
                    "UPDATE collection_songs SET position = ?1 \
                     WHERE collection_id = ?2 AND song_id = ?3 AND source = ?4",
                    params![position as i64, id, song_id, source],
                )?;
            }
            touch(transaction, id, &timestamp)?;
            Ok(())
        })?;
        Ok(())
    }

    /// Imports an external collection and its songs in one transaction.
    ///
    /// Atomic on purpose: a partially imported playlist is worse than no
    /// import, because the user cannot tell which tracks are missing. Any
    /// invalid song aborts the whole thing and leaves no collection behind.
    pub fn import(
        &self,
        name: &str,
        songs: &[Song],
        metadata: &ImportMetadata,
        content_type: CollectionType,
    ) -> Result<StoredCollection, CollectionStoreError> {
        let name = validate_name(name)?;
        if songs.is_empty() {
            return Err(CollectionStoreError::InvalidArgument(
                "导入的歌单没有曲目".to_owned(),
            ));
        }
        if songs.len() > MAX_IMPORT_SONGS {
            return Err(CollectionStoreError::InvalidArgument(format!(
                "导入的曲目过多，最多 {MAX_IMPORT_SONGS} 首"
            )));
        }
        // Validated before opening the transaction so a bad payload never
        // touches the database at all.
        for song in songs {
            validate_song(song)?;
        }

        let id = new_id("imp");
        let timestamp = now_iso8601();
        self.database.with_transaction(|transaction| {
            insert_collection_typed(
                transaction,
                &id,
                CollectionKind::Imported,
                content_type,
                &name,
                metadata,
                &timestamp,
            )?;
            let mut position = 0_i64;
            for song in songs {
                // Duplicates within one external playlist are skipped rather
                // than aborting: the source playlist may legitimately repeat.
                if insert_song(transaction, &id, song, position, &timestamp)? {
                    position += 1;
                }
            }
            Ok(())
        })?;

        self.get(&id)?
            .ok_or_else(|| CollectionStoreError::NotFound(id))
    }
}

/// Validates and normalizes a collection name.
fn validate_name(name: &str) -> Result<String, CollectionStoreError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CollectionStoreError::InvalidArgument(
            "歌单名称不能为空".to_owned(),
        ));
    }
    if trimmed.chars().count() > MAX_NAME_LENGTH {
        return Err(CollectionStoreError::InvalidArgument(format!(
            "歌单名称过长，最多 {MAX_NAME_LENGTH} 个字符"
        )));
    }
    Ok(trimmed.to_owned())
}

/// A song must be addressable, or it can never be removed or played again.
fn validate_song(song: &Song) -> Result<(), CollectionStoreError> {
    if song.reference.site_id.trim().is_empty() || song.reference.source.trim().is_empty() {
        return Err(CollectionStoreError::InvalidArgument(
            "歌曲引用无效".to_owned(),
        ));
    }
    if song.title.trim().is_empty() {
        return Err(CollectionStoreError::InvalidArgument(
            "歌曲标题不能为空".to_owned(),
        ));
    }
    Ok(())
}

/// Process-unique id; the millisecond clock plus a counter avoids a collision
/// when several collections are created inside one millisecond.
fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{sequence}", unix_millis())
}

fn insert_collection(
    connection: &Connection,
    id: &str,
    kind: CollectionKind,
    name: &str,
    metadata: &ImportMetadata,
    timestamp: &str,
) -> Result<(), rusqlite::Error> {
    insert_collection_typed(
        connection,
        id,
        kind,
        CollectionType::Playlist,
        name,
        metadata,
        timestamp,
    )
}

fn insert_collection_typed(
    connection: &Connection,
    id: &str,
    kind: CollectionKind,
    content_type: CollectionType,
    name: &str,
    metadata: &ImportMetadata,
    timestamp: &str,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO collections(\
            id, kind, content_type, source, external_id, name, description, creator, \
            cover_url, link, created_at, updated_at) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            id,
            kind.as_str(),
            content_type_str(content_type),
            metadata
                .source
                .clone()
                .unwrap_or_else(|| "local".to_owned()),
            metadata.external_id,
            name,
            metadata.description,
            metadata.creator,
            metadata.cover_url,
            metadata.link,
            timestamp,
        ],
    )?;
    Ok(())
}

/// Inserts a song; `false` means it was already in the collection.
fn insert_song(
    connection: &Connection,
    collection_id: &str,
    song: &Song,
    position: i64,
    timestamp: &str,
) -> Result<bool, rusqlite::Error> {
    let song_json = serde_json::to_string(song).map_err(|error| {
        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(error.to_string())))
    })?;
    let inserted = connection.execute(
        "INSERT INTO collection_songs(\
            collection_id, song_id, source, position, song_json, created_at) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(collection_id, song_id, source) DO NOTHING",
        params![
            collection_id,
            song.reference.site_id,
            song.reference.source,
            position,
            song_json,
            timestamp,
        ],
    )?;
    Ok(inserted > 0)
}

/// Next free position, so an append lands after existing songs.
fn next_position(connection: &Connection, collection_id: &str) -> Result<i64, rusqlite::Error> {
    connection.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM collection_songs WHERE collection_id = ?1",
        params![collection_id],
        |row| row.get(0),
    )
}

/// Current `(song_id, source)` keys in stored order.
fn current_keys(
    connection: &Connection,
    collection_id: &str,
) -> Result<Vec<(String, String)>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT song_id, source FROM collection_songs \
         WHERE collection_id = ?1 ORDER BY position ASC, song_id ASC",
    )?;
    let rows = statement.query_map(params![collection_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.collect()
}

fn touch(
    connection: &Connection,
    collection_id: &str,
    timestamp: &str,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "UPDATE collections SET updated_at = ?1 WHERE id = ?2",
        params![timestamp, collection_id],
    )?;
    Ok(())
}

fn content_type_str(content_type: CollectionType) -> &'static str {
    match content_type {
        CollectionType::Playlist => "playlist",
        CollectionType::Album => "album",
    }
}

fn read_collection(row: &Row<'_>) -> Result<StoredCollection, rusqlite::Error> {
    let kind: String = row.get(1)?;
    let content_type: String = row.get(2)?;
    Ok(StoredCollection {
        id: row.get(0)?,
        kind: CollectionKind::parse(&kind),
        content_type: if content_type == "album" {
            CollectionType::Album
        } else {
            CollectionType::Playlist
        },
        source: row.get(3)?,
        external_id: row.get(4)?,
        name: row.get(5)?,
        description: row.get(6)?,
        creator: row.get(7)?,
        cover_url: row.get(8)?,
        link: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        track_count: row.get::<_, i64>(12)?.max(0) as u64,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{CollectionKind, CollectionRepository, CollectionStoreError, ImportMetadata};
    use crate::domain::collection::CollectionType;
    use crate::domain::source::{Song, SongRef};
    use crate::infrastructure::storage::Database;

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

    fn repository() -> CollectionRepository {
        let database = Arc::new(Database::open_in_memory().expect("open"));
        CollectionRepository::new(database)
    }

    #[test]
    fn created_collection_is_listed_with_its_track_count() {
        let repository = repository();
        let created = repository.create("我的歌单", Some("说明")).expect("create");
        assert_eq!(created.kind, CollectionKind::User);
        assert_eq!(created.track_count, 0);

        repository
            .add_song(&created.id, &song("1", "A"))
            .expect("add");
        repository
            .add_song(&created.id, &song("2", "B"))
            .expect("add");

        let listed = repository.list().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "我的歌单");
        assert_eq!(listed[0].description.as_deref(), Some("说明"));
        assert_eq!(listed[0].track_count, 2);
    }

    #[test]
    fn blank_and_overlong_names_are_rejected() {
        let repository = repository();
        assert!(repository.create("   ", None).is_err());
        let long = "字".repeat(201);
        assert!(repository.create(&long, None).is_err());
        assert!(repository.list().expect("list").is_empty());
    }

    #[test]
    fn rename_persists_and_trims_the_name() {
        let repository = repository();
        let created = repository.create("旧名", None).expect("create");
        let renamed = repository.rename(&created.id, "  新名  ").expect("rename");
        assert_eq!(renamed.name, "新名");
        assert_eq!(
            repository
                .get(&created.id)
                .expect("get")
                .expect("some")
                .name,
            "新名"
        );
        assert!(repository.rename("missing", "x").is_err());
    }

    #[test]
    fn deleting_a_collection_removes_its_songs() {
        let repository = repository();
        let created = repository.create("歌单", None).expect("create");
        repository
            .add_song(&created.id, &song("1", "A"))
            .expect("add");

        repository.delete(&created.id).expect("delete");
        assert!(repository.get(&created.id).expect("get").is_none());
        assert!(repository.songs(&created.id).is_err());
        assert!(repository.delete(&created.id).is_err());
    }

    #[test]
    fn the_same_song_cannot_be_added_twice() {
        let repository = repository();
        let created = repository.create("歌单", None).expect("create");
        repository
            .add_song(&created.id, &song("7", "A"))
            .expect("add");

        let duplicate = repository.add_song(&created.id, &song("7", "A"));
        assert!(matches!(
            duplicate,
            Err(CollectionStoreError::DuplicateSong(_))
        ));
        assert_eq!(repository.songs(&created.id).expect("songs").len(), 1);
    }

    #[test]
    fn the_same_song_id_from_another_source_is_a_distinct_track() {
        let repository = repository();
        let created = repository.create("歌单", None).expect("create");
        repository
            .add_song(&created.id, &song("7", "A"))
            .expect("add");

        let mut other = song("7", "A");
        other.reference.source = "qq".into();
        repository.add_song(&created.id, &other).expect("add other");
        assert_eq!(repository.songs(&created.id).expect("songs").len(), 2);
    }

    #[test]
    fn batch_add_skips_duplicates_instead_of_failing() {
        let repository = repository();
        let created = repository.create("歌单", None).expect("create");
        repository
            .add_song(&created.id, &song("1", "A"))
            .expect("add");

        let outcome = repository
            .add_songs(
                &created.id,
                &[song("1", "A"), song("2", "B"), song("3", "C")],
            )
            .expect("batch");
        assert_eq!(
            (outcome.requested, outcome.added, outcome.skipped),
            (3, 2, 1)
        );
        assert_eq!(repository.songs(&created.id).expect("songs").len(), 3);
    }

    #[test]
    fn removing_a_song_only_affects_that_collection() {
        let repository = repository();
        let first = repository.create("A", None).expect("create");
        let second = repository.create("B", None).expect("create");
        repository
            .add_song(&first.id, &song("1", "X"))
            .expect("add");
        repository
            .add_song(&second.id, &song("1", "X"))
            .expect("add");

        repository
            .remove_song(&first.id, "1", "netease")
            .expect("remove");
        assert!(repository.songs(&first.id).expect("songs").is_empty());
        assert_eq!(repository.songs(&second.id).expect("songs").len(), 1);
        assert!(repository.remove_song(&first.id, "1", "netease").is_err());
    }

    #[test]
    fn import_stores_the_playlist_and_marks_it_imported() {
        let repository = repository();
        let imported = repository
            .import(
                "外部歌单",
                &[song("1", "A"), song("2", "B")],
                &ImportMetadata {
                    source: Some("netease".into()),
                    external_id: Some("9527".into()),
                    creator: Some("某人".into()),
                    ..Default::default()
                },
                CollectionType::Playlist,
            )
            .expect("import");

        assert_eq!(imported.kind, CollectionKind::Imported);
        assert_eq!(imported.track_count, 2);
        assert_eq!(imported.external_id.as_deref(), Some("9527"));
        assert_eq!(imported.source, "netease");

        let titles: Vec<_> = repository
            .songs(&imported.id)
            .expect("songs")
            .into_iter()
            .map(|song| song.title)
            .collect();
        assert_eq!(titles, ["A", "B"]);
    }

    #[test]
    fn import_leaves_nothing_behind_when_a_song_is_invalid() {
        let repository = repository();
        let mut broken = song("2", "B");
        broken.reference.site_id = "   ".into();

        let result = repository.import(
            "半个歌单",
            &[song("1", "A"), broken, song("3", "C")],
            &ImportMetadata::default(),
            CollectionType::Playlist,
        );
        assert!(result.is_err());
        // No collection row, and therefore no orphaned songs either.
        assert!(repository.list().expect("list").is_empty());
    }

    #[test]
    fn an_empty_import_is_rejected() {
        let repository = repository();
        assert!(repository
            .import(
                "空歌单",
                &[],
                &ImportMetadata::default(),
                CollectionType::Playlist
            )
            .is_err());
        assert!(repository.list().expect("list").is_empty());
    }

    #[test]
    fn imported_collections_reject_new_songs_but_allow_renaming() {
        let repository = repository();
        let imported = repository
            .import(
                "外部歌单",
                &[song("1", "A")],
                &ImportMetadata::default(),
                CollectionType::Playlist,
            )
            .expect("import");

        assert!(matches!(
            repository.add_song(&imported.id, &song("2", "B")),
            Err(CollectionStoreError::ImportedReadOnly(_))
        ));
        assert_eq!(repository.songs(&imported.id).expect("songs").len(), 1);
        assert_eq!(
            repository
                .rename(&imported.id, "本地命名")
                .expect("rename")
                .name,
            "本地命名"
        );
    }

    #[test]
    fn reorder_persists_the_new_order() {
        let repository = repository();
        let created = repository.create("歌单", None).expect("create");
        repository
            .add_songs(
                &created.id,
                &[song("1", "A"), song("2", "B"), song("3", "C")],
            )
            .expect("add");

        repository
            .reorder(
                &created.id,
                &[
                    ("3".to_owned(), "netease".to_owned()),
                    ("1".to_owned(), "netease".to_owned()),
                    ("2".to_owned(), "netease".to_owned()),
                ],
            )
            .expect("reorder");

        let order: Vec<_> = repository
            .songs(&created.id)
            .expect("songs")
            .into_iter()
            .map(|song| song.title)
            .collect();
        assert_eq!(order, ["C", "A", "B"]);
    }

    #[test]
    fn a_partial_reorder_keeps_unlisted_songs_without_losing_any() {
        let repository = repository();
        let created = repository.create("歌单", None).expect("create");
        repository
            .add_songs(
                &created.id,
                &[
                    song("1", "A"),
                    song("2", "B"),
                    song("3", "C"),
                    song("4", "D"),
                ],
            )
            .expect("add");

        // Only the last track is repositioned; unknown keys are ignored.
        repository
            .reorder(
                &created.id,
                &[
                    ("4".to_owned(), "netease".to_owned()),
                    ("999".to_owned(), "netease".to_owned()),
                ],
            )
            .expect("reorder");

        let order: Vec<_> = repository
            .songs(&created.id)
            .expect("songs")
            .into_iter()
            .map(|song| song.title)
            .collect();
        assert_eq!(order, ["D", "A", "B", "C"]);
    }

    #[test]
    fn appended_songs_land_after_a_reorder() {
        let repository = repository();
        let created = repository.create("歌单", None).expect("create");
        repository
            .add_songs(&created.id, &[song("1", "A"), song("2", "B")])
            .expect("add");
        repository
            .reorder(
                &created.id,
                &[
                    ("2".to_owned(), "netease".to_owned()),
                    ("1".to_owned(), "netease".to_owned()),
                ],
            )
            .expect("reorder");
        repository
            .add_song(&created.id, &song("3", "C"))
            .expect("add");

        let order: Vec<_> = repository
            .songs(&created.id)
            .expect("songs")
            .into_iter()
            .map(|song| song.title)
            .collect();
        assert_eq!(order, ["B", "A", "C"]);
    }

    #[test]
    fn reorder_of_a_missing_collection_fails() {
        let repository = repository();
        assert!(repository.reorder("missing", &[]).is_err());
    }
}
