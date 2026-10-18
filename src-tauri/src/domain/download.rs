use serde::{Deserialize, Serialize};

use super::source::Song;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub operation_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DownloadState {
    Queued,
    Resolving,
    Downloading,
    Processing,
    Completed,
    Failed,
    CancelRequested,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTask {
    pub id: String,
    pub song: Song,
    pub target_path: String,
    pub state: DownloadState,
    pub progress: f32,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_second: Option<u64>,
    /// True when a dedup hit meant no bytes were transferred.
    pub skipped: bool,
    /// Non-fatal note kept alongside a completed download, e.g. metadata
    /// embedding was skipped because ffmpeg is missing.
    pub warning: Option<String>,
    pub error: Option<DownloadError>,
    pub created_at: String,
    pub updated_at: String,
}

pub fn can_retry(state: DownloadState) -> bool {
    matches!(state, DownloadState::Failed)
}

#[cfg(test)]
mod tests {
    use super::{can_retry, DownloadState};

    #[test]
    fn only_failed_tasks_are_retryable() {
        assert!(can_retry(DownloadState::Failed));
        assert!(!can_retry(DownloadState::Completed));
        assert!(!can_retry(DownloadState::Cancelled));
    }
}
