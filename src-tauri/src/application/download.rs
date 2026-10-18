//! Parallel Range download engine.
//!
//! Probe with a 4 byte Range request, then either fan out fixed size chunks
//! or fall back to one unranged GET. Bytes land in a temporary file that is
//! fsynced and renamed, so a cancelled or failed download never leaves a
//! partial file at the real target path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Semaphore;

use crate::domain::audio::{content_type_is_opaque, detect_extension, mime_by_extension};
use crate::domain::download::{DownloadState, DownloadTask};
use crate::infrastructure::http::ByteFetcher;

/// First chunk is small so playback can start before the rest arrives.
const FIRST_CHUNK_SIZE: u64 = 32 * 1024;
const CHUNK_SIZE: u64 = 256 * 1024;
const MAX_CONCURRENT_CHUNKS: usize = 16;
const CHUNK_ATTEMPTS: u32 = 3;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(90);
const SINGLE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub task: DownloadTask,
}

#[async_trait]
pub trait Downloader: Send + Sync {
    async fn create(&self, request: DownloadRequest) -> Result<DownloadTask, String>;
    async fn cancel(&self, task_id: &str) -> Result<(), String>;
    async fn retry(&self, task_id: &str) -> Result<DownloadTask, String>;
}

pub fn next_state(
    current: DownloadState,
    requested: DownloadState,
) -> Result<DownloadState, String> {
    let allowed = matches!(
        (current, requested),
        (DownloadState::Queued, DownloadState::Resolving)
            | (DownloadState::Resolving, DownloadState::Downloading)
            | (DownloadState::Downloading, DownloadState::Processing)
            | (DownloadState::Processing, DownloadState::Completed)
            | (DownloadState::Queued, DownloadState::CancelRequested)
            | (DownloadState::Resolving, DownloadState::CancelRequested)
            | (DownloadState::Downloading, DownloadState::CancelRequested)
            | (DownloadState::Processing, DownloadState::CancelRequested)
            | (DownloadState::CancelRequested, DownloadState::Cancelled)
            | (DownloadState::Failed, DownloadState::Queued)
    );
    allowed
        .then_some(requested)
        .ok_or_else(|| format!("invalid download transition: {current:?} -> {requested:?}"))
}

/// Reason a transfer stopped, so callers can pick the IPC error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferError {
    /// Network or server problem; worth retrying.
    Network(String),
    /// The user cancelled.
    Cancelled,
    /// Local filesystem problem.
    Io(String),
}

impl TransferError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Network(_) => "DOWNLOAD_TRANSFER_FAILED",
            Self::Cancelled => "DOWNLOAD_CANCELLED",
            Self::Io(_) => "DOWNLOAD_WRITE_FAILED",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::Network(_))
    }

    pub fn message(&self) -> String {
        match self {
            Self::Network(detail) => format!("音频下载失败: {detail}"),
            Self::Cancelled => "下载已取消".to_owned(),
            Self::Io(detail) => format!("写入文件失败: {detail}"),
        }
    }
}

/// Cooperative cancellation shared between the command layer and the transfer.
///
/// Checked before each chunk dispatch and inside the assembly loop, so a cancel
/// stops in-flight work instead of only flipping the task state.
#[derive(Debug, Default, Clone)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Bytes downloaded so far, published for progress events.
#[derive(Debug, Default)]
pub struct TransferProgress {
    downloaded: AtomicU64,
    total: AtomicU64,
}

impl TransferProgress {
    pub fn downloaded(&self) -> u64 {
        self.downloaded.load(Ordering::Relaxed)
    }

    /// Total resource size; `None` until the probe resolves it.
    pub fn total(&self) -> Option<u64> {
        match self.total.load(Ordering::Relaxed) {
            0 => None,
            value => Some(value),
        }
    }

    fn add(&self, bytes: u64) {
        self.downloaded.fetch_add(bytes, Ordering::Relaxed);
    }

    fn set_total(&self, bytes: u64) {
        self.total.store(bytes, Ordering::Relaxed);
    }
}

/// A finished transfer: the audio bytes plus the resolved container metadata.
#[derive(Debug, Clone)]
pub struct AudioPayload {
    pub data: Vec<u8>,
    pub extension: &'static str,
    pub content_type: String,
}

/// Downloads `url`, choosing the parallel Range path when the server supports
/// it and a single unranged GET otherwise.
pub async fn fetch_audio(
    fetcher: &dyn ByteFetcher,
    url: &str,
    headers: &[(&str, String)],
    cancel: &CancelFlag,
    progress: &TransferProgress,
) -> Result<AudioPayload, TransferError> {
    if cancel.is_cancelled() {
        return Err(TransferError::Cancelled);
    }
    let probe = fetcher
        .fetch_range(url, headers, 0, Some(3), PROBE_TIMEOUT)
        .await
        .map_err(|error| TransferError::Network(error.to_string()))?;

    // 206 plus a parsable Content-Range total is the only case where chunking
    // is safe; anything else means the server ignored Range.
    match (probe.status, probe.total) {
        (206, Some(total)) if total > 0 => {
            progress.set_total(total);
            let head = probe.body.clone();
            let data =
                parallel_range(fetcher, url, headers, total, probe.body, cancel, progress).await?;
            Ok(payload(data, &head, probe.content_type.as_deref()))
        }
        _ => single(fetcher, url, headers, cancel, progress).await,
    }
}

async fn single(
    fetcher: &dyn ByteFetcher,
    url: &str,
    headers: &[(&str, String)],
    cancel: &CancelFlag,
    progress: &TransferProgress,
) -> Result<AudioPayload, TransferError> {
    if cancel.is_cancelled() {
        return Err(TransferError::Cancelled);
    }
    let response = fetcher
        .fetch_all(url, headers, SINGLE_TIMEOUT)
        .await
        .map_err(|error| TransferError::Network(error.to_string()))?;
    if !response.is_success() {
        return Err(TransferError::Network(format!(
            "unexpected status: {}",
            response.status
        )));
    }
    if cancel.is_cancelled() {
        return Err(TransferError::Cancelled);
    }
    let total = response.body.len() as u64;
    progress.set_total(total);
    progress.add(total);
    let head = response.body.clone();
    Ok(payload(
        response.body,
        &head,
        response.content_type.as_deref(),
    ))
}

/// Magic number wins over Content-Type, and an opaque Content-Type is replaced
/// by the extension-derived MIME type.
fn payload(data: Vec<u8>, head: &[u8], content_type: Option<&str>) -> AudioPayload {
    let extension = detect_extension(head, content_type);
    let content_type = match content_type {
        Some(value) if !content_type_is_opaque(Some(value)) => value.to_owned(),
        _ => mime_by_extension(extension).to_owned(),
    };
    AudioPayload {
        data,
        extension,
        content_type,
    }
}

/// Chunk boundaries after the probed prefix: one small chunk, then fixed size.
fn chunk_ranges(probed: u64, total: u64) -> Vec<(u64, u64)> {
    let mut ranges = Vec::new();
    let mut start = probed;
    let mut size = FIRST_CHUNK_SIZE;
    while start < total {
        let end = (start + size - 1).min(total - 1);
        ranges.push((start, end));
        start = end + 1;
        size = CHUNK_SIZE;
    }
    ranges
}

async fn parallel_range(
    fetcher: &dyn ByteFetcher,
    url: &str,
    headers: &[(&str, String)],
    total: u64,
    prefix: Vec<u8>,
    cancel: &CancelFlag,
    progress: &TransferProgress,
) -> Result<Vec<u8>, TransferError> {
    let mut data = Vec::with_capacity(total as usize);
    let probed = prefix.len() as u64;
    progress.add(probed);
    data.extend_from_slice(&prefix);
    if probed >= total {
        data.truncate(total as usize);
        return Ok(data);
    }

    let ranges = chunk_ranges(probed, total);
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_CHUNKS));
    let mut running = futures_util::stream::FuturesUnordered::new();
    let mut queued = ranges.iter().copied().enumerate();
    // Out-of-order results wait here until every earlier index has been written.
    let mut pending: HashMap<usize, Vec<u8>> = HashMap::new();
    let mut next_write = 0usize;

    loop {
        // Only ever hold a window of futures: a long song has thousands of
        // chunks and allocating them all upfront wastes memory for no gain.
        while running.len() < MAX_CONCURRENT_CHUNKS {
            let Some((index, (start, end))) = queued.next() else {
                break;
            };
            if cancel.is_cancelled() {
                return Err(TransferError::Cancelled);
            }
            let permits = Arc::clone(&permits);
            running.push(async move {
                let _permit = permits
                    .acquire()
                    .await
                    .map_err(|_| TransferError::Network("chunk scheduler closed".to_owned()))?;
                let bytes = fetch_chunk(fetcher, url, headers, start, end, cancel).await?;
                Ok::<(usize, Vec<u8>), TransferError>((index, bytes))
            });
        }
        if running.is_empty() {
            break;
        }

        let Some(result) = futures_util::StreamExt::next(&mut running).await else {
            break;
        };
        let (index, bytes) = result?;
        progress.add(bytes.len() as u64);
        pending.insert(index, bytes);
        while let Some(bytes) = pending.remove(&next_write) {
            data.extend_from_slice(&bytes);
            next_write += 1;
        }
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
    }

    // Any leftover means a chunk future vanished without an error, which would
    // silently produce a truncated file.
    while let Some(bytes) = pending.remove(&next_write) {
        data.extend_from_slice(&bytes);
        next_write += 1;
    }
    if next_write != ranges.len() {
        return Err(TransferError::Network(format!(
            "incomplete download: {next_write}/{} chunks",
            ranges.len()
        )));
    }
    Ok(data)
}

/// Fetches one chunk, retrying up to [`CHUNK_ATTEMPTS`] times.
///
/// A short read is a retry condition: some CDNs answer 206 and then close the
/// connection early, which would otherwise corrupt the assembled file.
async fn fetch_chunk(
    fetcher: &dyn ByteFetcher,
    url: &str,
    headers: &[(&str, String)],
    start: u64,
    end: u64,
    cancel: &CancelFlag,
) -> Result<Vec<u8>, TransferError> {
    let expected = (end - start + 1) as usize;
    let mut last = String::new();
    for _ in 0..CHUNK_ATTEMPTS {
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        match fetcher
            .fetch_range(url, headers, start, Some(end), CHUNK_TIMEOUT)
            .await
        {
            Err(error) => last = error.to_string(),
            Ok(response) => {
                if response.status != 206 && response.status != 200 {
                    last = format!("unexpected status: {}", response.status);
                    continue;
                }
                if response.body.len() != expected {
                    last = format!("short chunk: got {} want {expected}", response.body.len());
                    continue;
                }
                return Ok(response.body);
            }
        }
    }
    Err(TransferError::Network(format!(
        "chunk {start}-{end} failed after {CHUNK_ATTEMPTS} attempts: {last}"
    )))
}

/// Writes `data` to a sibling temp file, fsyncs it, then renames onto `target`.
pub async fn write_atomically(target: &Path, data: &[u8]) -> Result<(), TransferError> {
    let parent = target
        .parent()
        .ok_or_else(|| TransferError::Io("目标路径没有上级目录".to_owned()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| TransferError::Io(error.to_string()))?;
    let temp = temp_path(target);
    // Cleanup only; the error is propagated unchanged.
    write_temp(&temp, data).await.inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })?;
    if let Err(error) = tokio::fs::rename(&temp, target).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(TransferError::Io(error.to_string()));
    }
    Ok(())
}

async fn write_temp(temp: &Path, data: &[u8]) -> Result<(), TransferError> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(temp)
        .await
        .map_err(|error| TransferError::Io(error.to_string()))?;
    file.write_all(data)
        .await
        .map_err(|error| TransferError::Io(error.to_string()))?;
    file.sync_all()
        .await
        .map_err(|error| TransferError::Io(error.to_string()))
}

/// Temp name in the target directory so the rename stays on one filesystem.
pub fn temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    let parent = target.parent().unwrap_or(Path::new("."));
    parent.join(format!(".{name}.part"))
}

/// Shared limiter for whole-song downloads.
///
/// Rebuilt when the configured concurrency changes; tasks already holding a
/// permit finish under the old limit.
pub struct DownloadLimiter {
    permits: std::sync::Mutex<(usize, Arc<Semaphore>)>,
}

impl Default for DownloadLimiter {
    fn default() -> Self {
        Self::new(3)
    }
}

impl DownloadLimiter {
    pub fn new(concurrency: usize) -> Self {
        let concurrency = concurrency.clamp(1, 5);
        Self {
            permits: std::sync::Mutex::new((concurrency, Arc::new(Semaphore::new(concurrency)))),
        }
    }

    /// Semaphore sized for `concurrency`, replacing the old one on change.
    pub fn semaphore(&self, concurrency: usize) -> Arc<Semaphore> {
        let concurrency = concurrency.clamp(1, 5);
        let mut guard = match self.permits.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.0 != concurrency {
            *guard = (concurrency, Arc::new(Semaphore::new(concurrency)));
        }
        Arc::clone(&guard.1)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::infrastructure::http::{BytesResponse, HttpError};
    use std::sync::Mutex;

    /// Scripted [`ByteFetcher`] driving the engine without a network.
    pub struct FakeFetcher {
        body: Vec<u8>,
        /// Status the probe answers with; 206 enables the chunked path.
        pub probe_status: u16,
        /// `Content-Range` total reported by the probe.
        pub probe_total: Option<u64>,
        pub content_type: Option<String>,
        /// Number of leading chunk requests that fail, per range start.
        failures: Mutex<HashMap<u64, u32>>,
        /// Delay applied to a chunk so results arrive out of order.
        delays: HashMap<u64, Duration>,
        pub range_calls: Mutex<Vec<(u64, Option<u64>)>>,
        pub full_calls: Mutex<u32>,
        /// Cancelled when a chunk request is observed, for cancel tests.
        pub cancel_on_chunk: Mutex<Option<CancelFlag>>,
    }

    impl FakeFetcher {
        pub fn ranged(body: Vec<u8>) -> Self {
            let total = body.len() as u64;
            Self {
                body,
                probe_status: 206,
                probe_total: Some(total),
                content_type: Some("audio/mpeg".to_owned()),
                failures: Mutex::new(HashMap::new()),
                delays: HashMap::new(),
                range_calls: Mutex::new(Vec::new()),
                full_calls: Mutex::new(0),
                cancel_on_chunk: Mutex::new(None),
            }
        }

        /// Server that ignores Range and always answers 200 with the full body.
        pub fn unranged(body: Vec<u8>) -> Self {
            Self {
                probe_status: 200,
                probe_total: None,
                ..Self::ranged(body)
            }
        }

        pub fn with_content_type(mut self, value: Option<&str>) -> Self {
            self.content_type = value.map(str::to_owned);
            self
        }

        pub fn failing(self, start: u64, times: u32) -> Self {
            if let Ok(mut failures) = self.failures.lock() {
                failures.insert(start, times);
            }
            self
        }

        pub fn delaying(mut self, start: u64, delay: Duration) -> Self {
            self.delays.insert(start, delay);
            self
        }

        pub fn cancelling(self, cancel: &CancelFlag) -> Self {
            if let Ok(mut slot) = self.cancel_on_chunk.lock() {
                *slot = Some(cancel.clone());
            }
            self
        }

        fn take_failure(&self, start: u64) -> bool {
            let Ok(mut failures) = self.failures.lock() else {
                return false;
            };
            match failures.get_mut(&start) {
                Some(remaining) if *remaining > 0 => {
                    *remaining -= 1;
                    true
                }
                _ => false,
            }
        }
    }

    #[async_trait]
    impl ByteFetcher for FakeFetcher {
        async fn fetch_range(
            &self,
            _url: &str,
            _headers: &[(&str, String)],
            start: u64,
            end: Option<u64>,
            _timeout: Duration,
        ) -> Result<BytesResponse, HttpError> {
            if let Ok(mut calls) = self.range_calls.lock() {
                calls.push((start, end));
            }
            let probe = start == 0 && end == Some(3);
            if probe {
                let upper = (4).min(self.body.len());
                return Ok(BytesResponse {
                    status: self.probe_status,
                    body: self.body[..upper].to_vec(),
                    total: self.probe_total,
                    content_type: self.content_type.clone(),
                });
            }
            if let Ok(slot) = self.cancel_on_chunk.lock() {
                if let Some(cancel) = slot.as_ref() {
                    cancel.cancel();
                }
            }
            if let Some(delay) = self.delays.get(&start) {
                tokio::time::sleep(*delay).await;
            }
            if self.take_failure(start) {
                // Truncated body: exercises the short-read retry condition.
                return Ok(BytesResponse {
                    status: 206,
                    body: Vec::new(),
                    total: self.probe_total,
                    content_type: self.content_type.clone(),
                });
            }
            let upper = end
                .map(|value| (value as usize + 1).min(self.body.len()))
                .unwrap_or(self.body.len());
            Ok(BytesResponse {
                status: 206,
                body: self.body[start as usize..upper].to_vec(),
                total: self.probe_total,
                content_type: self.content_type.clone(),
            })
        }

        async fn fetch_all(
            &self,
            _url: &str,
            _headers: &[(&str, String)],
            _timeout: Duration,
        ) -> Result<BytesResponse, HttpError> {
            if let Ok(mut calls) = self.full_calls.lock() {
                *calls += 1;
            }
            Ok(BytesResponse {
                status: 200,
                body: self.body.clone(),
                total: None,
                content_type: self.content_type.clone(),
            })
        }
    }

    /// `n` bytes with an ID3 header, so magic-number detection yields mp3.
    pub fn mp3_body(len: usize) -> Vec<u8> {
        let mut body = b"ID3\x03".to_vec();
        body.resize(len.max(4), 0x5A);
        body
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{mp3_body, FakeFetcher};
    use super::{
        chunk_ranges, fetch_audio, next_state, temp_path, write_atomically, CancelFlag,
        DownloadLimiter, TransferError, TransferProgress, CHUNK_SIZE, FIRST_CHUNK_SIZE,
    };
    use crate::domain::download::DownloadState;
    use std::time::Duration;

    #[test]
    fn accepts_normal_download_path() {
        assert_eq!(
            next_state(DownloadState::Queued, DownloadState::Resolving),
            Ok(DownloadState::Resolving)
        );
        assert_eq!(
            next_state(DownloadState::Processing, DownloadState::Completed),
            Ok(DownloadState::Completed)
        );
    }

    #[test]
    fn rejects_completed_to_downloading() {
        assert!(next_state(DownloadState::Completed, DownloadState::Downloading).is_err());
    }

    #[test]
    fn first_chunk_is_small_then_fixed_size() {
        let total = 4 + FIRST_CHUNK_SIZE + CHUNK_SIZE + 10;
        let ranges = chunk_ranges(4, total);
        assert_eq!(ranges[0], (4, 4 + FIRST_CHUNK_SIZE - 1));
        assert_eq!(ranges[1].1 - ranges[1].0 + 1, CHUNK_SIZE);
        assert_eq!(ranges.last().map(|range| range.1), Some(total - 1));
    }

    #[tokio::test]
    async fn falls_back_to_single_get_when_range_is_ignored() {
        let body = mp3_body(200_000);
        let fetcher = FakeFetcher::unranged(body.clone());
        let progress = TransferProgress::default();
        let payload = fetch_audio(
            &fetcher,
            "http://example.test/a.mp3",
            &[],
            &CancelFlag::new(),
            &progress,
        )
        .await
        .expect("single get succeeds");
        assert_eq!(payload.data, body);
        assert_eq!(*fetcher.full_calls.lock().expect("lock"), 1);
        // Only the probe used a Range header.
        assert_eq!(fetcher.range_calls.lock().expect("lock").len(), 1);
        assert_eq!(progress.total(), Some(body.len() as u64));
    }

    #[tokio::test]
    async fn assembles_chunks_in_order_when_they_resolve_out_of_order() {
        let body = mp3_body((4 + FIRST_CHUNK_SIZE + CHUNK_SIZE * 2) as usize);
        // Delay the first chunk so later chunks finish first.
        let fetcher = FakeFetcher::ranged(body.clone()).delaying(4, Duration::from_millis(60));
        let progress = TransferProgress::default();
        let payload = fetch_audio(
            &fetcher,
            "http://example.test/a.mp3",
            &[],
            &CancelFlag::new(),
            &progress,
        )
        .await
        .expect("chunked download succeeds");
        assert_eq!(payload.data, body);
        assert_eq!(progress.downloaded(), body.len() as u64);
    }

    #[tokio::test]
    async fn retries_a_chunk_and_succeeds_on_the_third_attempt() {
        let body = mp3_body((4 + FIRST_CHUNK_SIZE + 32) as usize);
        let fetcher = FakeFetcher::ranged(body.clone()).failing(4, 2);
        let payload = fetch_audio(
            &fetcher,
            "http://example.test/a.mp3",
            &[],
            &CancelFlag::new(),
            &TransferProgress::default(),
        )
        .await
        .expect("third attempt succeeds");
        assert_eq!(payload.data, body);
        let retries = fetcher
            .range_calls
            .lock()
            .expect("lock")
            .iter()
            .filter(|call| call.0 == 4)
            .count();
        assert_eq!(retries, 3);
    }

    #[tokio::test]
    async fn reports_retryable_failure_after_three_attempts() {
        let body = mp3_body((4 + FIRST_CHUNK_SIZE + 32) as usize);
        let fetcher = FakeFetcher::ranged(body).failing(4, 99);
        let error = fetch_audio(
            &fetcher,
            "http://example.test/a.mp3",
            &[],
            &CancelFlag::new(),
            &TransferProgress::default(),
        )
        .await
        .expect_err("exhausted attempts fail");
        assert!(error.retryable());
        assert_eq!(error.code(), "DOWNLOAD_TRANSFER_FAILED");
        assert_eq!(
            fetcher
                .range_calls
                .lock()
                .expect("lock")
                .iter()
                .filter(|call| call.0 == 4)
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn magic_number_beats_a_wrong_content_type() {
        let mut body = b"fLaC".to_vec();
        body.resize(5_000, 0x11);
        let fetcher = FakeFetcher::ranged(body).with_content_type(Some("audio/mpeg"));
        let payload = fetch_audio(
            &fetcher,
            "http://example.test/a",
            &[],
            &CancelFlag::new(),
            &TransferProgress::default(),
        )
        .await
        .expect("download succeeds");
        assert_eq!(payload.extension, "flac");
    }

    #[tokio::test]
    async fn opaque_content_type_is_replaced_by_the_extension_mime() {
        let mut body = b"fLaC".to_vec();
        body.resize(5_000, 0x11);
        let fetcher = FakeFetcher::ranged(body).with_content_type(Some("application/octet-stream"));
        let payload = fetch_audio(
            &fetcher,
            "http://example.test/a",
            &[],
            &CancelFlag::new(),
            &TransferProgress::default(),
        )
        .await
        .expect("download succeeds");
        assert_eq!(payload.content_type, "audio/flac");
    }

    #[tokio::test]
    async fn cancelling_stops_the_transfer() {
        let cancel = CancelFlag::new();
        let body = mp3_body((4 + FIRST_CHUNK_SIZE + CHUNK_SIZE * 4) as usize);
        let fetcher = FakeFetcher::ranged(body).cancelling(&cancel);
        let error = fetch_audio(
            &fetcher,
            "http://example.test/a.mp3",
            &[],
            &cancel,
            &TransferProgress::default(),
        )
        .await
        .expect_err("cancelled transfer fails");
        assert_eq!(error, TransferError::Cancelled);
        assert!(!error.retryable());
    }

    #[tokio::test]
    async fn atomic_write_leaves_no_temp_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("nested").join("song.mp3");
        write_atomically(&target, b"payload")
            .await
            .expect("write succeeds");
        assert_eq!(
            std::fs::read(&target).expect("read back"),
            b"payload".to_vec()
        );
        assert!(!temp_path(&target).exists());
    }

    #[test]
    fn limiter_resizes_when_settings_change() {
        let limiter = DownloadLimiter::new(1);
        let first = limiter.semaphore(1);
        assert_eq!(first.available_permits(), 1);
        let second = limiter.semaphore(4);
        assert_eq!(second.available_permits(), 4);
        // Same value reuses the live semaphore instead of resetting permits.
        let permit = second.try_acquire().expect("permit");
        assert_eq!(limiter.semaphore(4).available_permits(), 3);
        drop(permit);
    }
}
