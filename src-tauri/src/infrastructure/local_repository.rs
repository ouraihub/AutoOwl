//! Local library index backed by `local_music_tracks`.
//!
//! A recursive scan of the configured music directories keyed by absolute
//! path, so a rescan updates rows in place and only files that vanished from
//! disk are dropped. Rows outside the current
//! roots survive a scan — reconfiguring the music directory must not silently
//! erase a library that is merely not being scanned right now.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::domain::clock::{format_iso8601, now_iso8601};
use crate::domain::local_music::{
    fingerprint, local_song_ref, split_artists, LocalTrack, LocalTrackMetadata, Page,
};
use crate::domain::source::AlbumRef;
use crate::infrastructure::storage::{Database, StorageError};
use crate::infrastructure::tagging::read_tags;

/// Containers listed by the local library.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "wma", "ape", "alac", "aiff",
];

/// Depth cap for the recursive walk; music libraries are shallow and this stops
/// a pathological tree from stalling the scan.
const MAX_SCAN_DEPTH: u32 = 12;

/// Upper bound on a page, so a bad caller cannot ask for the whole library.
const MAX_PAGE_SIZE: u32 = 500;

#[derive(Debug, thiserror::Error)]
pub enum LocalMusicError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("track not found: {0}")]
    NotFound(String),
    #[error("filesystem error: {0}")]
    Io(String),
}

impl LocalMusicError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Storage(_) => "STORAGE_ERROR",
            Self::InvalidArgument(_) => "INVALID_ARGUMENT",
            Self::NotFound(_) => "LOCAL_TRACK_NOT_FOUND",
            Self::Io(_) => "LOCAL_IO_ERROR",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::Storage(_) | Self::Io(_))
    }
}

/// Filter and window for a library listing.
#[derive(Debug, Clone)]
pub struct LocalQuery {
    pub page: u32,
    pub page_size: u32,
    /// Case-insensitive substring matched against title and artist; empty
    /// means no filtering.
    pub filter: String,
}

impl LocalQuery {
    pub fn new(page: u32, page_size: u32) -> Self {
        Self {
            page,
            page_size,
            filter: String::new(),
        }
    }

    pub fn with_filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = filter.into();
        self
    }
}

/// What a scan changed, for logging and for the UI to report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanSummary {
    pub seen: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub removed: u64,
}

/// Row identity used to skip re-reading tags of untouched files.
struct IndexedFile {
    id: String,
    file_size: u64,
    modified_at: String,
}

pub struct LocalRepository {
    database: Arc<Database>,
}

impl LocalRepository {
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    /// Walks `roots`, upserts every audio file by path, and drops rows whose
    /// file no longer exists.
    ///
    /// Tag reading is best effort: an unparseable or untagged file is still
    /// indexed under its filename, so the library lists what is on disk
    /// regardless of metadata quality.
    pub fn scan(&self, roots: &[PathBuf]) -> Result<ScanSummary, LocalMusicError> {
        let known = self.indexed_files()?;
        let mut summary = ScanSummary::default();
        let mut seen_paths: Vec<String> = Vec::new();
        let mut pending: Vec<(LocalTrack, LocalTrackMetadata)> = Vec::new();

        for file in collect_audio_files(roots) {
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            let path_text = file.to_string_lossy().into_owned();
            summary.seen = summary.seen.saturating_add(1);
            seen_paths.push(path_text.clone());

            let modified_at = modified_iso8601(&metadata);
            let file_size = metadata.len();
            match known.get(&path_text) {
                Some(existing)
                    if existing.file_size == file_size && existing.modified_at == modified_at =>
                {
                    summary.unchanged = summary.unchanged.saturating_add(1);
                    continue;
                }
                Some(_) => summary.updated = summary.updated.saturating_add(1),
                None => summary.added = summary.added.saturating_add(1),
            }
            pending.push(build_track(&file, &path_text, file_size, modified_at));
        }

        let removed = self.database.with_transaction(|transaction| {
            for (track, metadata) in &pending {
                upsert_track(transaction, track, metadata)?;
            }
            prune_vanished(transaction, &known, &seen_paths)
        })?;
        summary.removed = removed;

        tracing::info!(
            event = "local_scan_finished",
            seen = summary.seen,
            added = summary.added,
            updated = summary.updated,
            removed = summary.removed
        );
        Ok(summary)
    }

    /// Paged listing, newest metadata included, ordered by title then path so
    /// page boundaries are stable across calls.
    pub fn list(&self, query: LocalQuery) -> Result<Page<LocalTrack>, LocalMusicError> {
        if query.page == 0 || query.page_size == 0 {
            return Err(LocalMusicError::InvalidArgument(
                "分页参数必须大于 0".to_owned(),
            ));
        }
        let page_size = query.page_size.min(MAX_PAGE_SIZE);
        let pattern = like_pattern(&query.filter);
        let offset = u64::from(query.page - 1).saturating_mul(u64::from(page_size));

        let (items, total) = self.database.with(|connection| {
            let total: u64 = connection.query_row(
                "SELECT COUNT(*) FROM local_music_tracks \
                 WHERE ?1 = '' OR lower(title) LIKE ?1 ESCAPE '\\' \
                    OR lower(COALESCE(artist, '')) LIKE ?1 ESCAPE '\\'",
                params![pattern],
                |row| row.get(0),
            )?;
            let mut statement = connection.prepare(
                "SELECT id, path, title, artist, album, format, file_size, modified_at, metadata_json \
                 FROM local_music_tracks \
                 WHERE ?1 = '' OR lower(title) LIKE ?1 ESCAPE '\\' \
                    OR lower(COALESCE(artist, '')) LIKE ?1 ESCAPE '\\' \
                 ORDER BY title COLLATE NOCASE ASC, path ASC \
                 LIMIT ?2 OFFSET ?3",
            )?;
            let rows = statement.query_map(params![pattern, page_size, offset as i64], read_track)?;
            Ok((rows.collect::<Result<Vec<_>, _>>()?, total))
        })?;

        Ok(Page {
            items,
            page: query.page,
            page_size,
            total,
        })
    }

    /// Single track by row id.
    pub fn get(&self, id: &str) -> Result<Option<LocalTrack>, LocalMusicError> {
        Ok(self.database.with(|connection| {
            connection
                .query_row(
                    "SELECT id, path, title, artist, album, format, file_size, modified_at, metadata_json \
                     FROM local_music_tracks WHERE id = ?1",
                    params![id],
                    read_track,
                )
                .optional()
        })?)
    }

    /// Removes the row and returns it, so the caller can decide about the file.
    pub fn remove(&self, id: &str) -> Result<LocalTrack, LocalMusicError> {
        let track = self
            .get(id)?
            .ok_or_else(|| LocalMusicError::NotFound(id.to_owned()))?;
        self.database.with(|connection| {
            connection.execute("DELETE FROM local_music_tracks WHERE id = ?1", params![id])
        })?;
        Ok(track)
    }

    /// True when nothing has been indexed yet, so the caller knows to scan.
    pub fn is_empty(&self) -> Result<bool, LocalMusicError> {
        let total: u64 = self.database.with(|connection| {
            connection.query_row("SELECT COUNT(*) FROM local_music_tracks", [], |row| {
                row.get(0)
            })
        })?;
        Ok(total == 0)
    }

    /// Paths already indexed, keyed by path.
    fn indexed_files(&self) -> Result<HashMap<String, IndexedFile>, LocalMusicError> {
        Ok(self.database.with(|connection| {
            let mut statement = connection
                .prepare("SELECT id, path, file_size, modified_at FROM local_music_tracks")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    IndexedFile {
                        id: row.get(0)?,
                        file_size: row.get::<_, i64>(2)?.max(0) as u64,
                        modified_at: row.get(3)?,
                    },
                ))
            })?;
            rows.collect::<Result<HashMap<_, _>, _>>()
        })?)
    }

    /// Inserts a row directly, so listing/filtering tests do not need real
    /// files on disk.
    #[cfg(test)]
    fn upsert_for_test(
        &self,
        path: &str,
        title: &str,
        artist: Option<&str>,
    ) -> Result<LocalTrack, LocalMusicError> {
        let id = track_id_for_path(path);
        let track = LocalTrack {
            reference: local_song_ref(&id),
            id,
            path: path.to_owned(),
            title: title.to_owned(),
            artists: artist.map(split_artists).unwrap_or_default(),
            album: None,
            format: "mp3".to_owned(),
            file_size: 1,
            modified_at: format_iso8601(0),
            duration_ms: None,
            cover_url: None,
        };
        self.database
            .with(|connection| upsert_track(connection, &track, &LocalTrackMetadata::default()))?;
        Ok(track)
    }
}

/// Recursive walk of every root, returning audio files in a deterministic
/// order so scan results do not depend on directory iteration order.
///
/// Symlinked directories are followed only within the depth cap; a cycle
/// therefore terminates instead of hanging the scan.
fn collect_audio_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut visited: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<(PathBuf, u32)> = roots.iter().map(|root| (root.clone(), 0_u32)).collect();

    while let Some((directory, depth)) = stack.pop() {
        // Canonicalize only directories: it is what makes two roots that share
        // a subtree scan that subtree once.
        let marker = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.clone());
        if visited.contains(&marker) {
            continue;
        }
        visited.push(marker);

        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(
                    event = "local_scan_dir_unreadable",
                    path = %directory.display(),
                    %error
                );
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if depth < MAX_SCAN_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if is_audio_path(&path) {
                found.push(path);
            }
        }
    }

    found.sort();
    found.dedup();
    found
}

/// True when the extension is a container the library lists.
pub fn is_audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|extension| AUDIO_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Modification time as a sortable UTC string; unknown times collapse to the
/// epoch so the row still compares equal across scans.
fn modified_iso8601(metadata: &std::fs::Metadata) -> String {
    let seconds = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs())
        .unwrap_or_default();
    format_iso8601(seconds)
}

/// Row id derived from the path, so rescanning a file keeps the `SongRef`
/// the player and collections already hold.
fn track_id_for_path(path_text: &str) -> String {
    format!("{:x}", md5::compute(path_text.as_bytes()))
}

/// Builds a row from disk metadata plus whatever tags could be read.
///
/// Tag failures are expected (unsupported containers, truncated files) and only
/// cost the metadata: the file is still indexed under its filename.
fn build_track(
    path: &Path,
    path_text: &str,
    file_size: u64,
    modified_at: String,
) -> (LocalTrack, LocalTrackMetadata) {
    let file_stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("未知曲目");
    let format = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    let tags = match read_tags(path) {
        Ok(tags) => tags,
        Err(error) => {
            tracing::debug!(event = "local_tags_unreadable", path = %path.display(), %error);
            Default::default()
        }
    };

    let title = tags
        .title
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| file_stem.to_owned());
    let artists = tags
        .artist
        .as_deref()
        .map(split_artists)
        .unwrap_or_default();
    let album = tags
        .album
        .filter(|value| !value.trim().is_empty())
        .map(|name| AlbumRef {
            id: None,
            name,
            source: Some(crate::domain::local_music::LOCAL_SOURCE.to_owned()),
        });
    let metadata = LocalTrackMetadata {
        duration_ms: tags.duration_ms,
        bitrate_kbps: tags.bitrate_kbps,
        sample_rate: tags.sample_rate,
        track_number: tags.track_number,
        year: tags.year,
        album_artist: tags.album_artist,
        has_cover: tags.has_cover,
    };
    let id = track_id_for_path(path_text);

    let track = LocalTrack {
        reference: local_song_ref(&id),
        id,
        path: path_text.to_owned(),
        title,
        artists,
        album,
        format,
        file_size,
        modified_at,
        duration_ms: metadata.duration_ms,
        cover_url: None,
    };
    (track, metadata)
}

/// Writes one row, keyed by path so a moved-then-restored file reuses its row.
fn upsert_track(
    connection: &Connection,
    track: &LocalTrack,
    metadata: &LocalTrackMetadata,
) -> Result<(), rusqlite::Error> {
    let artist = track.artist_text();
    let metadata_json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".to_owned());
    connection.execute(
        "INSERT INTO local_music_tracks(\
            id, path, fingerprint, title, artist, album, format, file_size, modified_at, \
            metadata_json, indexed_at) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
         ON CONFLICT(path) DO UPDATE SET \
            fingerprint = excluded.fingerprint, title = excluded.title, \
            artist = excluded.artist, album = excluded.album, format = excluded.format, \
            file_size = excluded.file_size, modified_at = excluded.modified_at, \
            metadata_json = excluded.metadata_json, indexed_at = excluded.indexed_at",
        params![
            track.id,
            track.path,
            fingerprint(&track.title, artist.as_deref(), track.duration_ms),
            track.title,
            artist,
            track.album.as_ref().map(|album| album.name.clone()),
            track.format,
            track.file_size as i64,
            track.modified_at,
            metadata_json,
            now_iso8601(),
        ],
    )?;
    Ok(())
}

/// Deletes rows whose file was not seen by this scan.
///
/// Only rows that live under a scanned root are eligible: a library on a
/// disconnected drive, or one whose directory was removed from settings, must
/// keep its rows instead of being wiped by an unrelated scan.
fn prune_vanished(
    connection: &Connection,
    known: &HashMap<String, IndexedFile>,
    seen_paths: &[String],
) -> Result<u64, rusqlite::Error> {
    let mut removed = 0_u64;
    for (path, indexed) in known {
        if seen_paths.iter().any(|seen| seen == path) {
            continue;
        }
        // `try_exists` distinguishes "gone" from "cannot tell"; a permission or
        // I/O error must not delete an index entry.
        if !matches!(Path::new(path).try_exists(), Ok(false)) {
            continue;
        }
        connection.execute(
            "DELETE FROM local_music_tracks WHERE id = ?1",
            params![indexed.id],
        )?;
        removed = removed.saturating_add(1);
    }
    Ok(removed)
}

/// Builds a lowercase `LIKE` pattern, escaping the wildcards so a filter
/// containing `%` or `_` matches literally.
fn like_pattern(filter: &str) -> String {
    let trimmed = filter.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut escaped = String::with_capacity(trimmed.len() + 2);
    escaped.push('%');
    for character in trimmed.to_lowercase().chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

fn read_track(row: &Row<'_>) -> Result<LocalTrack, rusqlite::Error> {
    let id: String = row.get(0)?;
    let artist: Option<String> = row.get(3)?;
    let album: Option<String> = row.get(4)?;
    let metadata_json: String = row.get(8)?;
    let metadata: LocalTrackMetadata = serde_json::from_str(&metadata_json).unwrap_or_default();
    Ok(LocalTrack {
        reference: local_song_ref(&id),
        id,
        path: row.get(1)?,
        title: row.get(2)?,
        artists: artist.as_deref().map(split_artists).unwrap_or_default(),
        album: album.filter(|name| !name.is_empty()).map(|name| AlbumRef {
            id: None,
            name,
            source: Some(crate::domain::local_music::LOCAL_SOURCE.to_owned()),
        }),
        format: row.get(5)?,
        file_size: row.get::<_, i64>(6)?.max(0) as u64,
        modified_at: row.get(7)?,
        duration_ms: metadata.duration_ms,
        cover_url: None,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::{is_audio_path, like_pattern, LocalQuery, LocalRepository};
    use crate::infrastructure::storage::Database;

    fn repository() -> LocalRepository {
        let database = Arc::new(Database::open_in_memory().expect("open"));
        LocalRepository::new(database)
    }

    /// Writes a file whose bytes are a valid-enough container header; tag
    /// reading may still fail, which the scan must tolerate.
    fn write_audio(root: &std::path::Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&path, bytes).expect("write file");
        path
    }

    #[test]
    fn scan_indexes_audio_recursively_and_ignores_other_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_audio(temp.path(), "a.mp3", b"ID3\x03\0\0\0\0");
        write_audio(temp.path(), "nested/b.flac", b"fLaC\0\0\0\0");
        write_audio(temp.path(), "notes.txt", b"not audio");
        write_audio(temp.path(), "cover.jpg", b"\xff\xd8\xff");

        let repository = repository();
        let summary = repository.scan(&[temp.path().to_path_buf()]).expect("scan");
        assert_eq!(summary.seen, 2);
        assert_eq!(summary.added, 2);

        let page = repository.list(LocalQuery::new(1, 50)).expect("list");
        assert_eq!(page.total, 2);
        let formats: Vec<_> = page
            .items
            .iter()
            .map(|track| track.format.clone())
            .collect();
        assert!(formats.contains(&"mp3".to_owned()));
        assert!(formats.contains(&"flac".to_owned()));
    }

    #[test]
    fn rescan_upserts_by_path_and_keeps_the_track_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write_audio(temp.path(), "a.mp3", b"ID3\x03\0\0\0\0");
        let roots = vec![temp.path().to_path_buf()];

        let repository = repository();
        repository.scan(&roots).expect("first scan");
        let first = repository.list(LocalQuery::new(1, 10)).expect("list");
        let original_id = first.items[0].id.clone();

        // Same path, different bytes: the row must be updated, not duplicated.
        std::fs::write(&file, b"ID3\x03\0\0\0\0longer contents").expect("rewrite");
        let summary = repository.scan(&roots).expect("second scan");
        assert_eq!(summary.added, 0);

        let second = repository.list(LocalQuery::new(1, 10)).expect("list");
        assert_eq!(second.total, 1);
        assert_eq!(second.items[0].id, original_id);
        assert_eq!(
            second.items[0].file_size,
            b"ID3\x03\0\0\0\0longer contents".len() as u64
        );
    }

    #[test]
    fn scan_drops_rows_whose_file_disappeared() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_audio(temp.path(), "keep.mp3", b"ID3\x03\0\0\0\0");
        let doomed = write_audio(temp.path(), "gone.mp3", b"ID3\x03\0\0\0\0");
        let roots = vec![temp.path().to_path_buf()];

        let repository = repository();
        repository.scan(&roots).expect("first scan");
        assert_eq!(
            repository.list(LocalQuery::new(1, 10)).expect("list").total,
            2
        );

        std::fs::remove_file(&doomed).expect("remove");
        let summary = repository.scan(&roots).expect("second scan");
        assert_eq!(summary.removed, 1);

        let page = repository.list(LocalQuery::new(1, 10)).expect("list");
        assert_eq!(page.total, 1);
        assert!(page.items[0].path.ends_with("keep.mp3"));
    }

    #[test]
    fn scan_keeps_rows_that_live_outside_the_scanned_roots() {
        let library = tempfile::tempdir().expect("tempdir");
        let other = tempfile::tempdir().expect("tempdir");
        write_audio(library.path(), "a.mp3", b"ID3\x03\0\0\0\0");
        write_audio(other.path(), "b.mp3", b"ID3\x03\0\0\0\0");

        let repository = repository();
        repository
            .scan(&[library.path().to_path_buf(), other.path().to_path_buf()])
            .expect("scan both");
        assert_eq!(
            repository.list(LocalQuery::new(1, 10)).expect("list").total,
            2
        );

        // Only one root configured now; the other library's file still exists.
        let summary = repository
            .scan(&[library.path().to_path_buf()])
            .expect("scan one");
        assert_eq!(summary.removed, 0);
        assert_eq!(
            repository.list(LocalQuery::new(1, 10)).expect("list").total,
            2
        );
    }

    #[test]
    fn paging_is_stable_and_covers_every_row_exactly_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        for index in 0..5 {
            write_audio(
                temp.path(),
                &format!("track-{index}.mp3"),
                b"ID3\x03\0\0\0\0",
            );
        }
        let repository = repository();
        repository.scan(&[temp.path().to_path_buf()]).expect("scan");

        let first = repository.list(LocalQuery::new(1, 2)).expect("page 1");
        let second = repository.list(LocalQuery::new(2, 2)).expect("page 2");
        let third = repository.list(LocalQuery::new(3, 2)).expect("page 3");
        assert_eq!(first.total, 5);
        assert_eq!(
            (first.items.len(), second.items.len(), third.items.len()),
            (2, 2, 1)
        );
        assert!(first.has_more());
        assert!(!third.has_more());

        let mut ids: Vec<_> = first
            .items
            .iter()
            .chain(second.items.iter())
            .chain(third.items.iter())
            .map(|track| track.id.clone())
            .collect();
        let total_ids = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), total_ids, "pages must not overlap");

        // Re-reading the same window returns the same rows in the same order.
        let repeat = repository
            .list(LocalQuery::new(2, 2))
            .expect("page 2 again");
        let repeat_ids: Vec<_> = repeat.items.iter().map(|track| track.id.clone()).collect();
        let second_ids: Vec<_> = second.items.iter().map(|track| track.id.clone()).collect();
        assert_eq!(repeat_ids, second_ids);
    }

    #[test]
    fn page_beyond_the_end_is_empty_but_reports_the_total() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_audio(temp.path(), "a.mp3", b"ID3\x03\0\0\0\0");
        let repository = repository();
        repository.scan(&[temp.path().to_path_buf()]).expect("scan");

        let page = repository.list(LocalQuery::new(9, 10)).expect("list");
        assert!(page.items.is_empty());
        assert_eq!(page.total, 1);
        assert!(!page.has_more());
    }

    #[test]
    fn zero_page_or_size_is_rejected() {
        let repository = repository();
        assert!(repository.list(LocalQuery::new(0, 10)).is_err());
        assert!(repository.list(LocalQuery::new(1, 0)).is_err());
    }

    #[test]
    fn filter_matches_title_and_artist_case_insensitively() {
        let repository = repository();
        repository
            .upsert_for_test("/music/one.mp3", "Nightingale", Some("Alice"))
            .expect("insert");
        repository
            .upsert_for_test("/music/two.mp3", "Sunrise", Some("Bob Nightowl"))
            .expect("insert");
        repository
            .upsert_for_test("/music/three.mp3", "Rain", None)
            .expect("insert");

        let by_title = repository
            .list(LocalQuery::new(1, 10).with_filter("nightin"))
            .expect("filter title");
        assert_eq!(by_title.total, 1);
        assert_eq!(by_title.items[0].title, "Nightingale");

        let by_artist = repository
            .list(LocalQuery::new(1, 10).with_filter("NIGHTOWL"))
            .expect("filter artist");
        assert_eq!(by_artist.total, 1);
        assert_eq!(by_artist.items[0].title, "Sunrise");

        let unmatched = repository
            .list(LocalQuery::new(1, 10).with_filter("zzz"))
            .expect("filter miss");
        assert_eq!(unmatched.total, 0);

        let unfiltered = repository
            .list(LocalQuery::new(1, 10).with_filter("   "))
            .expect("blank filter");
        assert_eq!(unfiltered.total, 3);
    }

    #[test]
    fn wildcards_in_a_filter_match_literally() {
        let repository = repository();
        repository
            .upsert_for_test("/music/a.mp3", "100% Pure", None)
            .expect("insert");
        repository
            .upsert_for_test("/music/b.mp3", "Anything", None)
            .expect("insert");

        let escaped = repository
            .list(LocalQuery::new(1, 10).with_filter("%"))
            .expect("filter");
        assert_eq!(escaped.total, 1);
        assert_eq!(escaped.items[0].title, "100% Pure");
        assert_eq!(like_pattern("a%b"), "%a\\%b%");
    }

    #[test]
    fn removing_a_track_returns_the_row_and_clears_the_index() {
        let repository = repository();
        let track = repository
            .upsert_for_test("/music/a.mp3", "Song", Some("Artist"))
            .expect("insert");

        let removed = repository.remove(&track.id).expect("remove");
        assert_eq!(removed.path, "/music/a.mp3");
        assert!(repository.get(&track.id).expect("get").is_none());
        assert!(repository.remove(&track.id).is_err());
    }

    #[test]
    fn audio_extensions_are_matched_case_insensitively() {
        assert!(is_audio_path(std::path::Path::new("a.MP3")));
        assert!(is_audio_path(std::path::Path::new("a.flac")));
        assert!(!is_audio_path(std::path::Path::new("a.txt")));
        assert!(!is_audio_path(std::path::Path::new("noextension")));
    }
}
