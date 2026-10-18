//! Playback history backed by `playback_history`.
//!
//! Newest first, paged, clearable. Every append trims the table to
//! `RETENTION_LIMIT` rows so an always-on session cannot grow the database
//! without bound.

use std::sync::Arc;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::domain::clock::now_iso8601;
use crate::domain::local_music::Page;
use crate::domain::source::Song;
use crate::infrastructure::storage::{Database, StorageError};

/// Rows kept after a trim.
pub const RETENTION_LIMIT: u64 = 500;

/// Upper bound on a page, so a bad caller cannot pull the whole table.
const MAX_PAGE_SIZE: u32 = 200;

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl HistoryError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Storage(_) => "STORAGE_ERROR",
            Self::InvalidArgument(_) => "INVALID_ARGUMENT",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::Storage(_))
    }
}

/// One play, as the history modal renders it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackEntry {
    pub id: i64,
    pub song: Song,
    pub played_at: String,
}

pub struct HistoryRepository {
    database: Arc<Database>,
    retention: u64,
}

impl HistoryRepository {
    pub fn new(database: Arc<Database>) -> Self {
        Self {
            database,
            retention: RETENTION_LIMIT,
        }
    }

    /// Repository with a custom retention window, for tests.
    #[cfg(test)]
    fn with_retention(database: Arc<Database>, retention: u64) -> Self {
        Self {
            database,
            retention,
        }
    }

    /// Appends a play and trims the table back to the retention window.
    ///
    /// A repeat of the same song is a distinct row: the history is a log of what
    /// was played, not a set of distinct tracks.
    pub fn record(&self, song: &Song) -> Result<PlaybackEntry, HistoryError> {
        if song.reference.site_id.trim().is_empty() {
            return Err(HistoryError::InvalidArgument("歌曲引用无效".to_owned()));
        }
        let song_json = serde_json::to_string(song)
            .map_err(|error| HistoryError::InvalidArgument(format!("歌曲序列化失败: {error}")))?;
        let played_at = now_iso8601();
        let retention = self.retention;

        let id = self.database.with_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO playback_history(song_json, played_at) VALUES(?1, ?2)",
                params![song_json, played_at],
            )?;
            let id = transaction.last_insert_rowid();
            trim(transaction, retention)?;
            Ok(id)
        })?;

        Ok(PlaybackEntry {
            id,
            song: song.clone(),
            played_at,
        })
    }

    /// Newest first. Ties on `played_at` break by descending id, so entries
    /// recorded within the same second still page deterministically.
    pub fn list(&self, page: u32, page_size: u32) -> Result<Page<PlaybackEntry>, HistoryError> {
        if page == 0 || page_size == 0 {
            return Err(HistoryError::InvalidArgument(
                "分页参数必须大于 0".to_owned(),
            ));
        }
        let page_size = page_size.min(MAX_PAGE_SIZE);
        let offset = u64::from(page - 1).saturating_mul(u64::from(page_size));

        let (items, total) = self.database.with(|connection| {
            let total: u64 =
                connection.query_row("SELECT COUNT(*) FROM playback_history", [], |row| {
                    row.get(0)
                })?;
            let mut statement = connection.prepare(
                "SELECT id, song_json, played_at FROM playback_history \
                 ORDER BY played_at DESC, id DESC LIMIT ?1 OFFSET ?2",
            )?;
            let rows = statement.query_map(params![page_size, offset as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            Ok((rows.collect::<Result<Vec<_>, _>>()?, total))
        })?;

        // A row whose payload no longer deserializes is skipped rather than
        // failing the whole listing; the total still reflects stored rows.
        let items = items
            .into_iter()
            .filter_map(|(id, song_json, played_at)| {
                match serde_json::from_str::<Song>(&song_json) {
                    Ok(song) => Some(PlaybackEntry {
                        id,
                        song,
                        played_at,
                    }),
                    Err(error) => {
                        tracing::warn!(event = "history_row_unreadable", id, %error);
                        None
                    }
                }
            })
            .collect();

        Ok(Page {
            items,
            page,
            page_size,
            total,
        })
    }

    /// Empties the history; returns how many rows were dropped.
    pub fn clear(&self) -> Result<u64, HistoryError> {
        let removed = self
            .database
            .with(|connection| connection.execute("DELETE FROM playback_history", []))?;
        Ok(removed as u64)
    }

    /// Current row count.
    pub fn count(&self) -> Result<u64, HistoryError> {
        Ok(self.database.with(|connection| {
            connection.query_row("SELECT COUNT(*) FROM playback_history", [], |row| {
                row.get(0)
            })
        })?)
    }
}

/// Drops the oldest rows beyond `retention`.
///
/// Ordering matches `list`, so the rows kept are exactly the first page(s) a
/// user would see.
fn trim(connection: &Connection, retention: u64) -> Result<usize, rusqlite::Error> {
    connection.execute(
        "DELETE FROM playback_history WHERE id NOT IN (\
            SELECT id FROM playback_history ORDER BY played_at DESC, id DESC LIMIT ?1\
         )",
        params![retention as i64],
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{HistoryRepository, RETENTION_LIMIT};
    use crate::domain::source::{Artist, Song, SongRef};
    use crate::infrastructure::storage::Database;

    fn song(site_id: &str, title: &str) -> Song {
        Song {
            reference: SongRef {
                source: "netease".into(),
                scope: "song".into(),
                site_id: site_id.into(),
            },
            title: title.into(),
            artists: vec![Artist {
                id: None,
                name: "Artist".into(),
            }],
            album: None,
            duration_ms: Some(1000),
            cover_url: None,
        }
    }

    fn repository() -> HistoryRepository {
        let database = Arc::new(Database::open_in_memory().expect("open"));
        HistoryRepository::new(database)
    }

    #[test]
    fn history_is_newest_first_and_round_trips_the_song() {
        let repository = repository();
        for index in 0..3 {
            repository
                .record(&song(&index.to_string(), &format!("Track {index}")))
                .expect("record");
        }

        let page = repository.list(1, 10).expect("list");
        assert_eq!(page.total, 3);
        let titles: Vec<_> = page
            .items
            .iter()
            .map(|entry| entry.song.title.clone())
            .collect();
        assert_eq!(titles, ["Track 2", "Track 1", "Track 0"]);
        assert_eq!(page.items[0].song.reference.site_id, "2");
        assert_eq!(page.items[0].song.artists[0].name, "Artist");
    }

    #[test]
    fn repeated_plays_of_one_song_are_separate_entries() {
        let repository = repository();
        repository.record(&song("7", "Same")).expect("first");
        repository.record(&song("7", "Same")).expect("second");
        assert_eq!(repository.count().expect("count"), 2);
    }

    #[test]
    fn history_pages_without_overlap_or_gaps() {
        let repository = repository();
        for index in 0..5 {
            repository
                .record(&song(&index.to_string(), &format!("Track {index}")))
                .expect("record");
        }

        let first = repository.list(1, 2).expect("page 1");
        let second = repository.list(2, 2).expect("page 2");
        let third = repository.list(3, 2).expect("page 3");
        assert_eq!(
            (first.items.len(), second.items.len(), third.items.len()),
            (2, 2, 1)
        );
        assert!(first.has_more());
        assert!(!third.has_more());

        let ordered: Vec<_> = first
            .items
            .iter()
            .chain(second.items.iter())
            .chain(third.items.iter())
            .map(|entry| entry.song.reference.site_id.clone())
            .collect();
        assert_eq!(ordered, ["4", "3", "2", "1", "0"]);
    }

    #[test]
    fn zero_page_or_size_is_rejected() {
        let repository = repository();
        assert!(repository.list(0, 10).is_err());
        assert!(repository.list(1, 0).is_err());
    }

    #[test]
    fn appending_trims_to_the_retention_cap_and_keeps_the_newest() {
        let database = Arc::new(Database::open_in_memory().expect("open"));
        let repository = HistoryRepository::with_retention(database, 3);
        for index in 0..8 {
            repository
                .record(&song(&index.to_string(), &format!("Track {index}")))
                .expect("record");
        }

        assert_eq!(repository.count().expect("count"), 3);
        let page = repository.list(1, 10).expect("list");
        let kept: Vec<_> = page
            .items
            .iter()
            .map(|entry| entry.song.reference.site_id.clone())
            .collect();
        assert_eq!(kept, ["7", "6", "5"]);
    }

    #[test]
    fn clearing_empties_the_history_and_reports_the_row_count() {
        let repository = repository();
        repository.record(&song("1", "A")).expect("record");
        repository.record(&song("2", "B")).expect("record");

        assert_eq!(repository.clear().expect("clear"), 2);
        assert_eq!(repository.count().expect("count"), 0);
        assert!(repository.list(1, 10).expect("list").items.is_empty());
        // Clearing an empty history is not an error.
        assert_eq!(repository.clear().expect("clear again"), 0);
    }

    #[test]
    fn a_song_without_an_id_is_rejected() {
        let repository = repository();
        assert!(repository.record(&song("   ", "Blank")).is_err());
        assert_eq!(repository.count().expect("count"), 0);
    }

    #[test]
    fn default_retention_is_the_documented_window() {
        assert_eq!(RETENTION_LIMIT, 500);
    }
}
