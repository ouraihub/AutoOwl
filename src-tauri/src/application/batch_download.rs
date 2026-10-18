//! Batch download planning and per-song outcome reporting.
//!
//! A batch shares the single whole-song concurrency limit with individual
//! downloads and never aborts on an item failure: each track records
//! success/skipped/failed and the batch keeps going, so a dead source in the
//! middle of a playlist does not lose the rest of the queue.

use serde::{Deserialize, Serialize};

use crate::application::local_mutation::library_contains;
use crate::domain::download::{DownloadState, DownloadTask};
use crate::domain::local_music::LocalTrack;
use crate::domain::source::Song;

#[derive(Debug, Clone)]
pub struct BatchDownloadPlan {
    pub tasks: Vec<DownloadTask>,
    pub concurrency: usize,
}

impl BatchDownloadPlan {
    pub fn new(tasks: Vec<DownloadTask>, concurrency: usize) -> Result<Self, String> {
        if tasks.is_empty() {
            return Err("下载任务不能为空".to_owned());
        }
        validate_concurrency(concurrency)?;
        Ok(Self { tasks, concurrency })
    }
}

pub fn validate_concurrency(concurrency: usize) -> Result<(), String> {
    if !(1..=5).contains(&concurrency) {
        return Err("并发数必须在 1 到 5 之间".to_owned());
    }
    Ok(())
}

/// Rejects an empty request before any task ids are allocated.
pub fn validate_songs(songs: &[Song]) -> Result<(), String> {
    if songs.is_empty() {
        return Err("下载任务不能为空".to_owned());
    }
    Ok(())
}

/// What happened to one song in a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BatchOutcomeKind {
    Completed,
    /// Already downloaded to the same path; no bytes transferred.
    SkippedDuplicate,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemOutcome {
    pub task_id: String,
    pub song: Song,
    pub target_path: String,
    pub outcome: BatchOutcomeKind,
    /// Error code for `Failed`, absent otherwise.
    pub code: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchDownloadReport {
    pub items: Vec<BatchItemOutcome>,
    pub completed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub cancelled: usize,
}

impl BatchDownloadReport {
    pub fn from_items(items: Vec<BatchItemOutcome>) -> Self {
        let count =
            |kind: BatchOutcomeKind| items.iter().filter(|item| item.outcome == kind).count();
        Self {
            completed: count(BatchOutcomeKind::Completed),
            skipped: count(BatchOutcomeKind::SkippedDuplicate),
            failed: count(BatchOutcomeKind::Failed),
            cancelled: count(BatchOutcomeKind::Cancelled),
            items,
        }
    }
}

/// Classifies a finished task into its batch outcome.
pub fn outcome_of(task: &DownloadTask) -> BatchItemOutcome {
    let outcome = match task.state {
        DownloadState::Completed if task.skipped => BatchOutcomeKind::SkippedDuplicate,
        DownloadState::Completed => BatchOutcomeKind::Completed,
        DownloadState::Cancelled | DownloadState::CancelRequested => BatchOutcomeKind::Cancelled,
        _ => BatchOutcomeKind::Failed,
    };
    BatchItemOutcome {
        task_id: task.id.clone(),
        song: task.song.clone(),
        target_path: task.target_path.clone(),
        outcome,
        code: task.error.as_ref().map(|error| error.code.clone()),
        message: task.error.as_ref().map(|error| error.message.clone()),
    }
}

/// Largest precheck request accepted.
pub const MAX_PRECHECK_SONGS: usize = 20_000;

/// Longest accepted name or artist in a precheck request.
pub const MAX_PRECHECK_FIELD_LEN: usize = 500;

/// One queued song as the precheck sees it: only what the library match needs.
///
/// Deliberately not a `Song`: the precheck runs on a plain import list (a
/// pasted playlist) before anything has been resolved to a source id.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrecheckSong {
    pub name: String,
    #[serde(default)]
    pub artist: String,
}

/// How much of a queue is already on disk.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrecheckReport {
    pub total: usize,
    pub skipped: usize,
}

/// Rejects a precheck request the backend refuses to scan.
///
/// Bounded by count and by field length so a malformed paste cannot turn into a
/// multi-million-comparison scan of the library.
pub fn validate_precheck(songs: &[PrecheckSong]) -> Result<(), String> {
    if songs.len() > MAX_PRECHECK_SONGS {
        return Err(format!("歌曲数量不能超过 {MAX_PRECHECK_SONGS} 首"));
    }
    let too_long = |value: &str| value.chars().count() > MAX_PRECHECK_FIELD_LEN;
    if songs
        .iter()
        .any(|song| too_long(&song.name) || too_long(&song.artist))
    {
        return Err(format!(
            "歌名与歌手长度不能超过 {MAX_PRECHECK_FIELD_LEN} 个字符"
        ));
    }
    Ok(())
}

/// Counts queued songs already present in `library`.
///
/// The match is `library_contains`, the same rule the duplicate view uses, so a
/// song reported as skipped here is the one the downloader would skip.
pub fn precheck(songs: &[PrecheckSong], library: &[LocalTrack]) -> PrecheckReport {
    let skipped = songs
        .iter()
        .filter(|song| {
            let artist = Some(song.artist.trim()).filter(|value| !value.is_empty());
            library_contains(library, &song.name, artist)
        })
        .count();
    PrecheckReport {
        total: songs.len(),
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        outcome_of, validate_precheck, BatchDownloadPlan, BatchDownloadReport, BatchOutcomeKind,
        PrecheckSong, MAX_PRECHECK_FIELD_LEN, MAX_PRECHECK_SONGS,
    };
    use crate::domain::download::{DownloadError, DownloadState, DownloadTask};
    use crate::domain::source::{Song, SongRef};

    fn task(state: DownloadState, skipped: bool, error: Option<DownloadError>) -> DownloadTask {
        DownloadTask {
            id: "dl-1".into(),
            song: Song {
                reference: SongRef {
                    source: "netease".into(),
                    scope: "song".into(),
                    site_id: "1".into(),
                },
                title: "歌名".into(),
                artists: Vec::new(),
                album: None,
                duration_ms: None,
                cover_url: None,
            },
            target_path: "a.mp3".into(),
            state,
            progress: 1.0,
            downloaded_bytes: 0,
            total_bytes: None,
            speed_bytes_per_second: None,
            skipped,
            warning: None,
            error,
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    #[test]
    fn validates_concurrency() {
        assert!(BatchDownloadPlan::new(Vec::new(), 3).is_err());
        assert!(super::validate_concurrency(6).is_err());
        assert!(super::validate_concurrency(3).is_ok());
    }

    #[test]
    fn a_skipped_completion_is_reported_as_duplicate() {
        let outcome = outcome_of(&task(DownloadState::Completed, true, None));
        assert_eq!(outcome.outcome, BatchOutcomeKind::SkippedDuplicate);
        assert!(outcome.code.is_none());
    }

    #[test]
    fn a_failure_carries_its_code_and_message() {
        let outcome = outcome_of(&task(
            DownloadState::Failed,
            false,
            Some(DownloadError {
                code: "DOWNLOAD_TRANSFER_FAILED".into(),
                message: "音频下载失败".into(),
                retryable: true,
                operation_id: "download".into(),
            }),
        ));
        assert_eq!(outcome.outcome, BatchOutcomeKind::Failed);
        assert_eq!(outcome.code.as_deref(), Some("DOWNLOAD_TRANSFER_FAILED"));
        assert_eq!(outcome.message.as_deref(), Some("音频下载失败"));
    }

    #[test]
    fn report_counts_every_outcome_class() {
        let report = BatchDownloadReport::from_items(vec![
            outcome_of(&task(DownloadState::Completed, false, None)),
            outcome_of(&task(DownloadState::Completed, true, None)),
            outcome_of(&task(DownloadState::Failed, false, None)),
            outcome_of(&task(DownloadState::Cancelled, false, None)),
        ]);
        assert_eq!(report.completed, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.cancelled, 1);
        assert_eq!(report.items.len(), 4);
    }

    fn precheck_song(name: &str, artist: &str) -> PrecheckSong {
        PrecheckSong {
            name: name.to_owned(),
            artist: artist.to_owned(),
        }
    }

    #[test]
    fn precheck_accepts_the_limits_and_rejects_beyond_them() {
        let at_limit = vec![precheck_song("歌名", "歌手"); MAX_PRECHECK_SONGS];
        assert!(validate_precheck(&at_limit).is_ok());

        let mut over_limit = at_limit;
        over_limit.push(precheck_song("歌名", "歌手"));
        assert!(validate_precheck(&over_limit).is_err());
    }

    #[test]
    fn precheck_rejects_an_overlong_name_or_artist() {
        let long = "字".repeat(MAX_PRECHECK_FIELD_LEN + 1);
        let exact = "字".repeat(MAX_PRECHECK_FIELD_LEN);

        assert!(validate_precheck(&[precheck_song(&exact, &exact)]).is_ok());
        assert!(validate_precheck(&[precheck_song(&long, "歌手")]).is_err());
        assert!(validate_precheck(&[precheck_song("歌名", &long)]).is_err());
    }
}
