//! Download commands: task lifecycle, progress events and folder reveal.
//!
//! A command only registers state and spawns the transfer; the engine in
//! `application::download` does the byte moving. Task state lives in
//! `AppState::downloads` for the live view and is mirrored into SQLite on every
//! terminal transition so history survives a restart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::application::batch_download::{
    outcome_of, validate_songs, BatchDownloadReport, BatchItemOutcome,
};
use crate::application::download::{
    fetch_audio, next_state, temp_path, write_atomically, CancelFlag, DownloadLimiter,
    TransferError, TransferProgress,
};
use crate::application::lyrics::{lrc_text, write_lrc_file};
use crate::domain::clock::now_iso8601;
use crate::domain::download::{can_retry, DownloadError, DownloadState, DownloadTask};
use crate::domain::filename;
use crate::domain::media::Lyrics;
use crate::domain::source::{Song, SongRef};
use crate::error::IpcError;
use crate::infrastructure::download_repository::DownloadRepository;
use crate::infrastructure::http::{source_headers, ByteFetcher};
use crate::infrastructure::tagging::{embed_metadata, CoverArt, EmbedOutcome, MetadataPatch};
use crate::state::AppState;

/// Progress events are throttled to ~10/sec per task; the UI cannot use more
/// and an unthrottled 256KiB chunk stream floods the IPC channel.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Rolling window for the speed estimate.
const SPEED_WINDOW: Duration = Duration::from_secs(3);
const PROGRESS_POLL: Duration = Duration::from_millis(50);
/// Cover art is a nice-to-have on an already-finished download, so it gets a
/// short deadline rather than the transfer's generous one.
const COVER_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Serialize)]
struct DownloadChanged {
    task: DownloadTask,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    id: String,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    progress: f32,
    speed_bytes_per_second: Option<u64>,
}

/// Cancellation flags for in-flight transfers, keyed by task id.
///
/// Process global rather than a field on `AppState` because that type is owned
/// by another slice; entries are removed as soon as a transfer settles.
static CANCELS: LazyLock<Mutex<HashMap<String, CancelFlag>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whole-song concurrency limiter, resized when settings change.
static LIMITER: LazyLock<DownloadLimiter> = LazyLock::new(DownloadLimiter::default);

#[tauri::command]
pub async fn create_download(
    app: AppHandle,
    state: State<'_, AppState>,
    song: Song,
) -> Result<DownloadTask, IpcError> {
    let task = register(&app, state.inner(), song)?;
    if task.skipped {
        return Ok(task);
    }
    let handle = app.clone();
    let id = task.id.clone();
    // The transfer outlives this command so the UI is not blocked; state and
    // events carry the result.
    tauri::async_runtime::spawn(async move {
        run_task(handle, id).await;
    });
    Ok(task)
}

#[tauri::command]
pub async fn create_batch_download(
    app: AppHandle,
    state: State<'_, AppState>,
    songs: Vec<Song>,
) -> Result<BatchDownloadReport, IpcError> {
    validate_songs(&songs).map_err(|message| error("INVALID_ARGUMENT", &message, false))?;
    let mut ids = Vec::with_capacity(songs.len());
    let mut immediate: Vec<BatchItemOutcome> = Vec::new();
    for song in songs {
        match register(&app, state.inner(), song) {
            Ok(task) if task.skipped => immediate.push(outcome_of(&task)),
            Ok(task) => ids.push(task.id),
            Err(failure) => return Err(failure),
        }
    }

    let mut running = Vec::with_capacity(ids.len());
    for id in ids {
        let handle = app.clone();
        running.push(tauri::async_runtime::spawn(async move {
            run_task(handle, id.clone()).await;
            id
        }));
    }

    let mut items = immediate;
    for handle in running {
        // A panicking transfer must not lose the rest of the batch.
        let Ok(id) = handle.await else {
            continue;
        };
        if let Some(task) = snapshot(&app, &id) {
            items.push(outcome_of(&task));
        }
    }
    Ok(BatchDownloadReport::from_items(items))
}

#[tauri::command]
pub fn cancel_download(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), IpcError> {
    // Signal the transfer first: it observes the flag between chunks and stops
    // before the state machine reaches Cancelled.
    if let Ok(flags) = CANCELS.lock() {
        if let Some(flag) = flags.get(&task_id) {
            flag.cancel();
        }
    }
    let task = {
        let mut tasks = lock_tasks(state.inner())?;
        let task = tasks
            .get_mut(&task_id)
            .ok_or_else(|| error("DOWNLOAD_NOT_FOUND", "下载任务不存在", false))?;
        let requested = next_state(task.state, DownloadState::CancelRequested)
            .map_err(|_| error("INVALID_DOWNLOAD_STATE", "当前任务不能取消", false))?;
        task.state = requested;
        task.updated_at = now_iso8601();
        task.clone()
    };
    emit_changed(&app, &task);
    Ok(())
}

#[tauri::command]
pub async fn retry_download(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<DownloadTask, IpcError> {
    let task = {
        let mut tasks = lock_tasks(state.inner())?;
        let task = tasks
            .get_mut(&task_id)
            .ok_or_else(|| error("DOWNLOAD_NOT_FOUND", "下载任务不存在", false))?;
        if !can_retry(task.state) {
            return Err(error("INVALID_DOWNLOAD_STATE", "当前任务不能重试", false));
        }
        // Restart from scratch: a failed transfer keeps no partial bytes.
        task.state = DownloadState::Queued;
        task.progress = 0.0;
        task.downloaded_bytes = 0;
        task.total_bytes = None;
        task.speed_bytes_per_second = None;
        task.error = None;
        task.warning = None;
        task.skipped = false;
        task.updated_at = now_iso8601();
        task.clone()
    };
    emit_changed(&app, &task);
    let handle = app.clone();
    let id = task.id.clone();
    tauri::async_runtime::spawn(async move {
        run_task(handle, id).await;
    });
    Ok(task)
}

#[tauri::command]
pub fn get_download_task(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<DownloadTask, IpcError> {
    lock_tasks(state.inner())?
        .get(&task_id)
        .cloned()
        .ok_or_else(|| error("DOWNLOAD_NOT_FOUND", "下载任务不存在", false))
}

#[tauri::command]
pub fn list_download_tasks(state: State<'_, AppState>) -> Result<Vec<DownloadTask>, IpcError> {
    let mut tasks: Vec<_> = lock_tasks(state.inner())?.values().cloned().collect();
    tasks.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then(right.id.cmp(&left.id))
    });
    Ok(tasks)
}

/// Reveals a finished download in the system file manager.
///
/// Falls back to the download root when the file is gone, so the command still
/// does something useful after a manual delete.
#[tauri::command]
pub fn open_download_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: Option<String>,
) -> Result<String, IpcError> {
    use tauri_plugin_opener::OpenerExt;
    let root = state.download_root();
    let target = match task_id {
        Some(id) => {
            let path = lock_tasks(state.inner())?
                .get(&id)
                .map(|task| PathBuf::from(&task.target_path))
                .ok_or_else(|| error("DOWNLOAD_NOT_FOUND", "下载任务不存在", false))?;
            if path.exists() {
                path
            } else {
                root.clone()
            }
        }
        None => root.clone(),
    };
    if !target.exists() {
        std::fs::create_dir_all(&root)
            .map_err(|failure| error("DOWNLOAD_OPEN_FAILED", &failure.to_string(), false))?;
    }
    let revealed = target.to_string_lossy().into_owned();
    if target.is_dir() {
        app.opener().open_path(revealed.clone(), None::<&str>)
    } else {
        app.opener().reveal_item_in_dir(&target)
    }
    .map_err(|failure| error("DOWNLOAD_OPEN_FAILED", &failure.to_string(), false))?;
    Ok(revealed)
}

/// Creates the task, applies the dedup check and stores it.
///
/// The extension is unknown until the first bytes arrive, so the initial target
/// path assumes `mp3`; `run_task` rewrites it once the container is detected.
fn register(app: &AppHandle, state: &AppState, song: Song) -> Result<DownloadTask, IpcError> {
    let settings = state.settings_snapshot();
    let target = filename::target(
        &state.download_root(),
        &settings.download_filename_template,
        &song,
        "mp3",
    );
    let target_path = target.to_string_lossy().into_owned();
    let duplicate = state
        .database
        .with(|connection| {
            DownloadRepository::is_duplicate(connection, &song.reference, &target_path)
        })
        .unwrap_or(false);

    let now = now_iso8601();
    let mut task = DownloadTask {
        id: state.next_task_id("dl"),
        song,
        target_path,
        state: DownloadState::Queued,
        progress: 0.0,
        downloaded_bytes: 0,
        total_bytes: None,
        speed_bytes_per_second: None,
        skipped: false,
        warning: None,
        error: None,
        created_at: now.clone(),
        updated_at: now,
    };
    if duplicate {
        // A dedup hit is a completed task that moved no bytes, so the UI can
        // show it in history without a spurious failure.
        task.state = DownloadState::Completed;
        task.progress = 1.0;
        task.skipped = true;
        task.warning = Some(format!(
            "已下载过，跳过：{}",
            crate::domain::similarity::song_key(
                task.song
                    .artists
                    .first()
                    .map(|artist| artist.name.as_str())
                    .unwrap_or(""),
                &task.song.title,
            )
        ));
    }
    store(state, &task)?;
    persist(state, &task);
    emit_changed(app, &task);
    Ok(task)
}

/// Runs one transfer end to end, publishing every state transition.
async fn run_task(app: AppHandle, id: String) {
    let state = app.state::<AppState>();
    let Some(task) = read_task(state.inner(), &id) else {
        return;
    };
    let settings = state.settings_snapshot();
    let permits = LIMITER.semaphore(settings.download_concurrency as usize);
    let Ok(_permit) = permits.acquire().await else {
        return;
    };

    let cancel = CancelFlag::new();
    if let Ok(mut flags) = CANCELS.lock() {
        flags.insert(id.clone(), cancel.clone());
    }
    // A cancel that landed while the task waited for a permit must be honoured.
    if matches!(
        read_task(state.inner(), &id).map(|task| task.state),
        Some(DownloadState::CancelRequested) | None
    ) {
        finish(&app, &id, Err(TransferError::Cancelled));
        return;
    }

    advance(&app, &id, DownloadState::Resolving);
    let outcome = transfer(&app, state.inner(), &task, &cancel, &id).await;
    finish(&app, &id, outcome);
}

/// Resolves the download URL, moves the bytes and writes the file.
async fn transfer(
    app: &AppHandle,
    state: &AppState,
    task: &DownloadTask,
    cancel: &CancelFlag,
    id: &str,
) -> Result<Completed, TransferError> {
    // Downloads resolve on the requested source only: the source's own
    // download URL is used and its error returned. Switching source is a
    // separate user-driven action (`switch_source`), not an automatic
    // fallback, so a failure here must surface as that source's failure.
    let reference = task.song.reference.clone();
    let source = state
        .sources
        .get(&reference.source)
        .ok_or_else(|| TransferError::Network(format!("音源不可用: {}", reference.source)))?;
    let resource = source
        .download_url(reference.clone())
        .await
        .map_err(|failure| TransferError::Network(failure.message))?;
    if resource.url.trim().is_empty() {
        return Err(TransferError::Network("下载地址为空".to_owned()));
    }

    advance(app, id, DownloadState::Downloading);
    let headers = source_headers(&reference.source, &state.cookies.get(&reference.source));
    let progress = Arc::new(TransferProgress::default());
    let reporter = spawn_progress(app.clone(), id.to_owned(), Arc::clone(&progress));
    let result = fetch_audio(
        &state.http,
        &resource.url,
        &headers,
        cancel,
        progress.as_ref(),
    )
    .await;
    reporter.abort();
    let payload = result?;

    advance(app, id, DownloadState::Processing);
    let settings = state.settings_snapshot();
    let target = filename::target(
        &state.download_root(),
        &settings.download_filename_template,
        &task.song,
        payload.extension,
    );
    if cancel.is_cancelled() {
        return Err(TransferError::Cancelled);
    }
    write_atomically(&target, &payload.data).await?;
    if settings.embed_download {
        enrich(state, task, &target, &reference).await;
    }
    Ok(Completed {
        target,
        extension: payload.extension,
        bytes: payload.data.len() as u64,
    })
}

/// Writes lyrics, cover and tags alongside a finished download.
///
/// Lyrics, then cover, then the embed, with each of the three treated as
/// best effort (a failure is dropped or downgraded to a warning). The
/// audio is already committed to disk when this runs, so propagating any of it
/// would turn a usable file into a failed download.
async fn enrich(state: &AppState, task: &DownloadTask, target: &Path, reference: &SongRef) {
    let lyrics = fetch_lyrics(state, reference).await;
    let cover = fetch_cover(state, task.song.cover_url.as_deref(), &reference.source).await;
    // An empty timeline renders to an empty string, and embedding that would
    // clear whatever lyrics the source already baked into the audio.
    let text = lyrics
        .as_ref()
        .map(lrc_text)
        .filter(|text| !text.trim().is_empty());
    let patch = metadata_patch(&task.song, text, cover);
    let path = target.to_path_buf();

    // lofty rewrites the container in place, which for a large flac is far too
    // much blocking work for a runtime worker; the sidecar write rides along so
    // the whole filesystem phase costs one hop.
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        if let Some(lyrics) = lyrics {
            if let Err(problem) = write_lrc_file(&path, &lyrics) {
                tracing::warn!(
                    event = "download_lrc_write_failed",
                    path = %path.display(),
                    %problem
                );
            }
        }
        embed_metadata(&path, &patch)
    })
    .await;

    match outcome {
        Ok(Ok(EmbedOutcome::Written)) => {
            tracing::debug!(event = "download_tags_written", path = %target.display());
        }
        Ok(Ok(EmbedOutcome::Skipped(reason))) => {
            tracing::debug!(event = "download_tags_skipped", reason);
        }
        Ok(Err(problem)) => {
            tracing::warn!(
                event = "download_tag_write_failed",
                path = %target.display(),
                %problem
            );
        }
        Err(_) => {
            tracing::warn!(event = "download_tag_write_panicked", path = %target.display());
        }
    }
}

/// Fetches lyrics for embedding and sidecar export; `None` when unavailable.
async fn fetch_lyrics(state: &AppState, reference: &SongRef) -> Option<Lyrics> {
    let source = state.sources.get(&reference.source)?;
    match source.lyrics(reference.clone()).await {
        Ok(lyrics) => Some(lyrics),
        Err(problem) => {
            // Sources without the capability answer
            // `SOURCE_CAPABILITY_UNSUPPORTED`, so an absent lyric is routine
            // rather than something worth warning about.
            tracing::debug!(
                event = "download_lyrics_unavailable",
                source = %reference.source,
                code = %problem.code
            );
            None
        }
    }
}

/// Downloads cover art for embedding; `None` whenever it cannot be had.
///
/// Sends the source's own headers: several art CDNs reject a request that
/// arrives without the matching Referer.
async fn fetch_cover(state: &AppState, cover_url: Option<&str>, source: &str) -> Option<CoverArt> {
    let url = cover_url.map(str::trim).filter(|url| !url.is_empty())?;
    let headers = source_headers(source, &state.cookies.get(source));
    let response = state
        .http
        .fetch_all(url, &headers, COVER_TIMEOUT)
        .await
        .inspect_err(|problem| {
            tracing::warn!(event = "download_cover_fetch_failed", source, %problem);
        })
        .ok()?;
    if !response.is_success() || response.body.is_empty() {
        tracing::warn!(
            event = "download_cover_rejected",
            source,
            status = response.status,
            bytes = response.body.len()
        );
        return None;
    }
    Some(CoverArt {
        data: response.body,
        mime: response.content_type,
    })
}

/// Maps a song plus the fetched extras onto the tag patch.
///
/// Artists join with `/`, which `domain::local_music::split_artists` treats as
/// a separator, so a tagged file re-indexed by the library scan recovers the
/// same performer list instead of collapsing into one bogus name. Blank names
/// are dropped so a source padding its collaborator list cannot leave a
/// dangling separator in the file.
fn metadata_patch(song: &Song, lyrics: Option<String>, cover: Option<CoverArt>) -> MetadataPatch {
    let artist = song
        .artists
        .iter()
        .map(|artist| artist.name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    MetadataPatch {
        title: (!song.title.trim().is_empty()).then(|| song.title.clone()),
        artist: (!artist.is_empty()).then_some(artist),
        album: song.album.as_ref().map(|album| album.name.clone()),
        lyrics,
        cover,
    }
}

/// Emits throttled progress events until aborted.
///
/// Polling a shared counter keeps the chunk loop free of event plumbing and
/// naturally coalesces the 16 concurrent chunk completions into one event.
fn spawn_progress(
    app: AppHandle,
    id: String,
    progress: Arc<TransferProgress>,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut last_emit = Instant::now() - PROGRESS_INTERVAL;
        let mut window: Vec<(Instant, u64)> = Vec::new();
        let mut previous = u64::MAX;
        loop {
            tokio::time::sleep(PROGRESS_POLL).await;
            let downloaded = progress.downloaded();
            if downloaded == previous || last_emit.elapsed() < PROGRESS_INTERVAL {
                continue;
            }
            previous = downloaded;
            last_emit = Instant::now();
            window.push((last_emit, downloaded));
            window.retain(|(at, _)| at.elapsed() <= SPEED_WINDOW);
            let total = progress.total();
            let event = DownloadProgress {
                id: id.clone(),
                downloaded_bytes: downloaded,
                total_bytes: total,
                progress: ratio(downloaded, total),
                speed_bytes_per_second: speed(&window),
            };
            // Mirror into the task so a polling `get_download_task` agrees with
            // the event stream.
            if let Ok(mut tasks) = app.state::<AppState>().downloads.lock() {
                if let Some(task) = tasks.get_mut(&id) {
                    task.downloaded_bytes = downloaded;
                    task.total_bytes = total;
                    task.progress = event.progress;
                    task.speed_bytes_per_second = event.speed_bytes_per_second;
                }
            }
            if let Err(failure) = app.emit("download:progress", event) {
                tracing::warn!(event = "download_progress_event_failed", %failure);
            }
        }
    })
}

/// Bytes per second across the sampling window.
fn speed(window: &[(Instant, u64)]) -> Option<u64> {
    let (first_at, first_bytes) = *window.first()?;
    let (last_at, last_bytes) = *window.last()?;
    let seconds = last_at.duration_since(first_at).as_secs_f64();
    if seconds <= 0.0 || last_bytes <= first_bytes {
        return None;
    }
    Some(((last_bytes - first_bytes) as f64 / seconds) as u64)
}

fn ratio(downloaded: u64, total: Option<u64>) -> f32 {
    match total {
        Some(total) if total > 0 => (downloaded as f64 / total as f64).min(1.0) as f32,
        _ => 0.0,
    }
}

/// Applies an intermediate transition, ignoring one that the state machine
/// rejects because a cancel already moved the task on.
fn advance(app: &AppHandle, id: &str, requested: DownloadState) {
    let state = app.state::<AppState>();
    let updated = {
        let Ok(mut tasks) = state.downloads.lock() else {
            return;
        };
        let Some(task) = tasks.get_mut(id) else {
            return;
        };
        match next_state(task.state, requested) {
            Ok(next) => {
                task.state = next;
                task.updated_at = now_iso8601();
                Some(task.clone())
            }
            Err(_) => None,
        }
    };
    if let Some(task) = updated {
        emit_changed(app, &task);
    }
}

/// A successful transfer's outcome.
struct Completed {
    target: PathBuf,
    extension: &'static str,
    bytes: u64,
}

/// Applies the terminal transition, cleans up and persists history.
fn finish(app: &AppHandle, id: &str, result: Result<Completed, TransferError>) {
    let state = app.state::<AppState>();
    if let Ok(mut flags) = CANCELS.lock() {
        flags.remove(id);
    }
    let task = {
        let Ok(mut tasks) = state.downloads.lock() else {
            return;
        };
        let Some(task) = tasks.get_mut(id) else {
            return;
        };
        match result {
            Ok(completed) => {
                task.target_path = completed.target.to_string_lossy().into_owned();
                task.state = DownloadState::Completed;
                task.progress = 1.0;
                task.downloaded_bytes = completed.bytes;
                task.total_bytes = Some(completed.bytes);
                task.error = None;
                tracing::info!(
                    event = "download_completed",
                    id,
                    extension = completed.extension,
                    bytes = completed.bytes
                );
            }
            Err(problem) => {
                task.state = if problem == TransferError::Cancelled {
                    DownloadState::Cancelled
                } else {
                    DownloadState::Failed
                };
                task.error = Some(failure(&problem));
                // Nothing may survive at the real path, and the temp file for
                // this task's target is now garbage.
                let _ = std::fs::remove_file(temp_path(Path::new(&task.target_path)));
            }
        }
        task.speed_bytes_per_second = None;
        task.updated_at = now_iso8601();
        task.clone()
    };
    if task.state == DownloadState::Completed {
        remember(state.inner(), &task.song.reference, &task.target_path);
    }
    persist(state.inner(), &task);
    emit_changed(app, &task);
}

fn failure(problem: &TransferError) -> DownloadError {
    DownloadError {
        code: problem.code().to_owned(),
        message: problem.message(),
        retryable: problem.retryable(),
        operation_id: "download".to_owned(),
    }
}

fn remember(state: &AppState, reference: &SongRef, target_path: &str) {
    if let Err(problem) = state
        .database
        .with(|connection| DownloadRepository::remember(connection, reference, target_path))
    {
        tracing::warn!(event = "download_dedup_write_failed", %problem);
    }
}

/// Mirrors the task into SQLite; a history write failure never fails a download.
fn persist(state: &AppState, task: &DownloadTask) {
    if let Err(problem) = state
        .database
        .with(|connection| DownloadRepository::upsert(connection, task))
    {
        tracing::warn!(event = "download_history_write_failed", %problem);
    }
}

fn store(state: &AppState, task: &DownloadTask) -> Result<(), IpcError> {
    lock_tasks(state)?.insert(task.id.clone(), task.clone());
    Ok(())
}

fn read_task(state: &AppState, id: &str) -> Option<DownloadTask> {
    state.downloads.lock().ok()?.get(id).cloned()
}

fn snapshot(app: &AppHandle, id: &str) -> Option<DownloadTask> {
    read_task(app.state::<AppState>().inner(), id)
}

fn lock_tasks<'a>(
    state: &'a AppState,
) -> Result<std::sync::MutexGuard<'a, HashMap<String, DownloadTask>>, IpcError> {
    state
        .downloads
        .lock()
        .map_err(|_| error("STATE_UNAVAILABLE", "下载任务暂时不可用", true))
}

fn emit_changed(app: &AppHandle, task: &DownloadTask) {
    if let Err(problem) = app.emit("download:changed", DownloadChanged { task: task.clone() }) {
        tracing::warn!(event = "download_event_failed", %problem);
    }
}

fn error(code: &str, message: &str, retryable: bool) -> IpcError {
    IpcError::new(code, message, retryable, "download".to_owned())
}

#[cfg(test)]
mod tests {
    use super::metadata_patch;
    use crate::domain::local_music::split_artists;
    use crate::domain::source::{AlbumRef, Artist, Song, SongRef};

    #[test]
    fn patch_joins_artists_so_a_rescan_recovers_them() {
        let song = Song {
            reference: SongRef {
                source: "netease".into(),
                scope: "song".into(),
                site_id: "1".into(),
            },
            title: "曲名".into(),
            artists: vec![
                Artist {
                    id: None,
                    name: "甲".into(),
                },
                // A padded entry must not leave a dangling separator behind.
                Artist {
                    id: None,
                    name: "  ".into(),
                },
                Artist {
                    id: None,
                    name: " 乙 ".into(),
                },
            ],
            album: Some(AlbumRef {
                id: None,
                name: "专辑".into(),
                source: None,
            }),
            duration_ms: None,
            cover_url: None,
        };

        let patch = metadata_patch(&song, Some("[00:01.00]词".into()), None);

        assert_eq!(patch.title.as_deref(), Some("曲名"));
        assert_eq!(patch.artist.as_deref(), Some("甲/乙"));
        assert_eq!(patch.album.as_deref(), Some("专辑"));
        assert_eq!(patch.lyrics.as_deref(), Some("[00:01.00]词"));

        // The written artist string must survive the library scan that reads
        // the file back, otherwise a downloaded track re-indexes as one
        // performer named "甲/乙".
        let recovered: Vec<String> = split_artists(patch.artist.as_deref().unwrap_or_default())
            .into_iter()
            .map(|artist| artist.name)
            .collect();
        assert_eq!(recovered, vec!["甲".to_owned(), "乙".to_owned()]);
    }
}
