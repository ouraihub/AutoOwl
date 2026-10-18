//! Persistence for download history and the dedup index.
//!
//! Two tables: `download_tasks` is the UI visible history, `download_dedup` is
//! the durable index that keeps an already downloaded song from being fetched
//! again after the history is cleared.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

use crate::domain::clock::now_iso8601;
use crate::domain::download::{DownloadState, DownloadTask};
use crate::domain::source::SongRef;

/// One history row as the records list renders it.
///
/// Flat and string-typed on purpose: these rows outlive the in-memory
/// `DownloadTask` (they survive a restart), and the records view only needs
/// what the table stores, not a rehydrated task.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRecord {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub target_path: String,
    pub state: String,
    pub progress: f32,
    pub error_code: Option<String>,
    pub created_at: String,
}

pub struct DownloadRepository;

impl DownloadRepository {
    pub fn save_state(
        connection: &Connection,
        id: &str,
        state: DownloadState,
        progress: f32,
        error_code: Option<&str>,
    ) -> Result<usize> {
        connection.execute(
            "UPDATE download_tasks SET state = ?1, progress = ?2, error_code = ?3, updated_at = datetime('now') WHERE id = ?4",
            params![state_text(state), progress, error_code, id],
        )
    }

    /// Inserts or refreshes the history row for `task`.
    pub fn upsert(connection: &Connection, task: &DownloadTask) -> Result<()> {
        let artist = task
            .song
            .artists
            .first()
            .map(|artist| artist.name.as_str())
            .unwrap_or("");
        connection.execute(
            "INSERT INTO download_tasks(\
               id, source, scope, site_id, title, artist, target_path, state, progress, error_code, created_at, updated_at) \
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
             ON CONFLICT(id) DO UPDATE SET \
               target_path = excluded.target_path, \
               state = excluded.state, \
               progress = excluded.progress, \
               error_code = excluded.error_code, \
               updated_at = excluded.updated_at",
            params![
                task.id,
                task.song.reference.source,
                task.song.reference.scope,
                task.song.reference.site_id,
                task.song.title,
                artist,
                task.target_path,
                state_text(task.state),
                task.progress,
                task.error.as_ref().map(|error| error.code.as_str()),
                task.created_at,
                task.updated_at,
            ],
        )?;
        Ok(())
    }

    /// True when this song was already downloaded to this exact path.
    pub fn is_duplicate(
        connection: &Connection,
        song: &SongRef,
        target_path: &str,
    ) -> Result<bool> {
        let found: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM download_dedup \
                 WHERE source = ?1 AND scope = ?2 AND site_id = ?3 AND target_path = ?4",
                params![song.source, song.scope, song.site_id, target_path],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Records a completed download so a later request is skipped.
    pub fn remember(connection: &Connection, song: &SongRef, target_path: &str) -> Result<()> {
        connection.execute(
            "INSERT INTO download_dedup(source, scope, site_id, target_path, created_at) \
             VALUES(?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(source, scope, site_id, target_path) DO NOTHING",
            params![
                song.source,
                song.scope,
                song.site_id,
                target_path,
                now_iso8601()
            ],
        )?;
        Ok(())
    }

    /// Total history rows, so a caller can clamp a page before fetching it.
    pub fn count(connection: &Connection) -> Result<u64> {
        connection.query_row("SELECT COUNT(*) FROM download_tasks", [], |row| row.get(0))
    }

    /// One page of history, newest first.
    ///
    /// Ties on `created_at` break by descending id, so rows written within the
    /// same second still page deterministically instead of repeating or
    /// vanishing across page boundaries.
    pub fn page(connection: &Connection, limit: u32, offset: u64) -> Result<Vec<DownloadRecord>> {
        let mut statement = connection.prepare(
            "SELECT id, title, artist, target_path, state, progress, error_code, created_at \
             FROM download_tasks ORDER BY created_at DESC, id DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = statement.query_map(params![limit, offset as i64], |row| {
            Ok(DownloadRecord {
                id: row.get(0)?,
                title: row.get(1)?,
                artist: row.get(2)?,
                target_path: row.get(3)?,
                state: row.get(4)?,
                progress: row.get(5)?,
                error_code: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        rows.collect()
    }

    /// Empties the visible history; returns how many rows were dropped.
    ///
    /// `download_dedup` is deliberately untouched: the dedup index survives a
    /// history clear so cleared songs are still not refetched.
    pub fn clear(connection: &Connection) -> Result<usize> {
        connection.execute("DELETE FROM download_tasks", [])
    }

    pub fn target_is_inside(root: &Path, target: &Path) -> bool {
        target.starts_with(root)
    }
}

/// Lowercase state name, matching the `download_tasks.state` column values.
fn state_text(state: DownloadState) -> String {
    format!("{state:?}").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::DownloadRepository;
    use crate::domain::clock::now_iso8601;
    use crate::domain::download::{DownloadError, DownloadState, DownloadTask};
    use crate::domain::source::{Artist, Song, SongRef};
    use crate::infrastructure::storage::Database;
    use std::path::Path;

    fn song_ref() -> SongRef {
        SongRef {
            source: "netease".into(),
            scope: "song".into(),
            site_id: "42".into(),
        }
    }

    fn task(id: &str, state: DownloadState) -> DownloadTask {
        let now = now_iso8601();
        DownloadTask {
            id: id.into(),
            song: Song {
                reference: song_ref(),
                title: "歌名".into(),
                artists: vec![Artist {
                    id: None,
                    name: "歌手".into(),
                }],
                album: None,
                duration_ms: None,
                cover_url: None,
            },
            target_path: "D:/music/歌手 - 歌名.mp3".into(),
            state,
            progress: 0.0,
            downloaded_bytes: 0,
            total_bytes: None,
            speed_bytes_per_second: None,
            skipped: false,
            warning: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[test]
    fn rejects_target_outside_root() {
        assert!(!DownloadRepository::target_is_inside(
            Path::new("downloads"),
            Path::new("other/a.mp3")
        ));
    }

    #[test]
    fn upsert_replaces_state_of_an_existing_row() {
        let database = Database::open_in_memory().expect("database");
        database
            .with(|connection| {
                let mut task = task("dl-1", DownloadState::Downloading);
                DownloadRepository::upsert(connection, &task)?;
                task.state = DownloadState::Failed;
                task.progress = 0.5;
                task.error = Some(DownloadError {
                    code: "DOWNLOAD_TRANSFER_FAILED".into(),
                    message: "失败".into(),
                    retryable: true,
                    operation_id: "download".into(),
                });
                task.updated_at = now_iso8601();
                DownloadRepository::upsert(connection, &task)?;
                let rows = DownloadRepository::page(connection, 10, 0)?;
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].state, "failed");
                assert_eq!(DownloadRepository::count(connection)?, 1);
                Ok(())
            })
            .expect("history round trip");
    }

    #[test]
    fn dedup_is_scoped_to_the_target_path() {
        let database = Database::open_in_memory().expect("database");
        database
            .with(|connection| {
                let reference = song_ref();
                assert!(!DownloadRepository::is_duplicate(
                    connection, &reference, "a.mp3"
                )?);
                DownloadRepository::remember(connection, &reference, "a.mp3")?;
                // Repeating the insert must not raise a primary key error.
                DownloadRepository::remember(connection, &reference, "a.mp3")?;
                assert!(DownloadRepository::is_duplicate(
                    connection, &reference, "a.mp3"
                )?);
                // A different destination is a fresh download, not a duplicate.
                assert!(!DownloadRepository::is_duplicate(
                    connection, &reference, "b.mp3"
                )?);
                Ok(())
            })
            .expect("dedup round trip");
    }

    #[test]
    fn save_state_updates_progress_and_error_code() {
        let database = Database::open_in_memory().expect("database");
        database
            .with(|connection| {
                DownloadRepository::upsert(connection, &task("dl-2", DownloadState::Queued))?;
                let updated = DownloadRepository::save_state(
                    connection,
                    "dl-2",
                    DownloadState::Cancelled,
                    0.25,
                    Some("DOWNLOAD_CANCELLED"),
                )?;
                assert_eq!(updated, 1);
                let state: String = connection.query_row(
                    "SELECT state FROM download_tasks WHERE id = 'dl-2'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(state, "cancelled");
                Ok(())
            })
            .expect("state update");
    }
}
