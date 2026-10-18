//! Playback history and download record commands.
//!
//! Backs the history modal: append a play, page through it newest-first, clear
//! it. The repository trims itself on every append, so no command exposes a
//! retention knob. The download records here are the same kind of view over
//! `download_tasks`: a persisted log the user can page through and wipe.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::application::local_music::clamp_page;
use crate::domain::local_music::Page;
use crate::domain::source::Song;
use crate::error::IpcError;
use crate::infrastructure::download_repository::{DownloadRecord, DownloadRepository};
use crate::infrastructure::history_repository::{HistoryError, HistoryRepository, PlaybackEntry};
use crate::state::AppState;

/// Default and ceiling for a records page.
const RECORDS_PAGE_SIZE: u32 = 20;
const MAX_RECORDS_PAGE_SIZE: u32 = 100;

#[tauri::command]
pub fn record_playback(state: State<'_, AppState>, song: Song) -> Result<PlaybackEntry, IpcError> {
    repository(&state)
        .record(&song)
        .map_err(|error| ipc(&error, "record_playback"))
}

#[tauri::command]
pub fn list_playback_history(
    state: State<'_, AppState>,
    page: u32,
    page_size: u32,
) -> Result<Page<PlaybackEntry>, IpcError> {
    repository(&state)
        .list(page, page_size)
        .map_err(|error| ipc(&error, "list_playback_history"))
}

#[tauri::command]
pub fn clear_playback_history(state: State<'_, AppState>) -> Result<u64, IpcError> {
    repository(&state)
        .clear()
        .map_err(|error| ipc(&error, "clear_playback_history"))
}

/// One page of persisted download records.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRecordsPage {
    pub records: Vec<DownloadRecord>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
    pub total_pages: u32,
}

/// Download history rows, newest first.
///
/// A page past the end is clamped to the last page rather than returning an
/// empty list, so a UI still holding page 5 after a clear lands on page 1
/// instead of showing nothing.
#[tauri::command]
pub fn list_download_records(
    state: State<'_, AppState>,
    page: u32,
    page_size: u32,
) -> Result<DownloadRecordsPage, IpcError> {
    let page_size = if page_size == 0 {
        RECORDS_PAGE_SIZE
    } else {
        page_size.min(MAX_RECORDS_PAGE_SIZE)
    };

    let (records, page, total, total_pages) = state
        .database
        .with(|connection| {
            let total = DownloadRepository::count(connection)?;
            let (page, pages) = clamp_page(page, total, page_size);
            let offset = u64::from(page - 1) * u64::from(page_size);
            Ok((
                DownloadRepository::page(connection, page_size, offset)?,
                page,
                total,
                pages,
            ))
        })
        .map_err(|error| ipc(&HistoryError::Storage(error), "list_download_records"))?;

    Ok(DownloadRecordsPage {
        records,
        page,
        page_size,
        total,
        total_pages,
    })
}

/// Drops every download record; returns how many rows were removed.
#[tauri::command]
pub fn clear_download_records(state: State<'_, AppState>) -> Result<u64, IpcError> {
    let removed = state
        .database
        .with(DownloadRepository::clear)
        .map_err(|error| ipc(&HistoryError::Storage(error), "clear_download_records"))?;
    Ok(removed as u64)
}

fn repository(state: &State<'_, AppState>) -> HistoryRepository {
    HistoryRepository::new(Arc::clone(&state.database))
}

fn ipc(error: &HistoryError, operation: &str) -> IpcError {
    tracing::warn!(event = "history_command_failed", operation, %error);
    IpcError::new(
        error.code(),
        &error.to_string(),
        error.retryable(),
        operation.to_owned(),
    )
}
