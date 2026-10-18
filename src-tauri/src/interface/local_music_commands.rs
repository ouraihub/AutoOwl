//! Local library commands.
//!
//! Every command resolves the configured music roots from settings, so the
//! scanned library follows the download directory the user chose.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::application::batch_download::{
    precheck, validate_precheck, PrecheckReport, PrecheckSong,
};
use crate::application::local_music::{music_roots, total_pages, LocalMusicService};
use crate::application::local_mutation::duplicate_groups;
use crate::domain::local_music::{LocalTrack, Page};
use crate::error::IpcError;
use crate::infrastructure::local_repository::LocalMusicError;
use crate::state::AppState;

/// Default and ceiling for a duplicates page: a group is tall, so the page is
/// small.
const DUPLICATES_PAGE_SIZE: u32 = 10;
const MAX_DUPLICATES_PAGE_SIZE: u32 = 50;

/// What a refresh changed, for the UI's toast.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshReport {
    pub seen: u64,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
}

#[tauri::command]
pub fn get_local_music(
    state: State<'_, AppState>,
    page: u32,
    page_size: u32,
    filter: Option<String>,
) -> Result<Page<LocalTrack>, IpcError> {
    let roots = roots(&state);
    service(&state)
        .list(
            &roots,
            page,
            page_size,
            filter.as_deref().unwrap_or_default(),
        )
        .map_err(|error| ipc(&error, "get_local_music"))
}

#[tauri::command]
pub fn refresh_local_music(state: State<'_, AppState>) -> Result<RefreshReport, IpcError> {
    let roots = roots(&state);
    let summary = service(&state)
        .refresh(&roots)
        .map_err(|error| ipc(&error, "refresh_local_music"))?;
    Ok(RefreshReport {
        seen: summary.seen,
        added: summary.added,
        updated: summary.updated,
        removed: summary.removed,
    })
}

/// Deletes a track from the index, and from disk when `delete_file`.
///
/// Defaults to deleting the file, but the guard in the use case still refuses
/// a path outside the library.
#[tauri::command]
pub fn delete_local_music(
    state: State<'_, AppState>,
    id: String,
    delete_file: Option<bool>,
) -> Result<LocalTrack, IpcError> {
    let roots = roots(&state);
    service(&state)
        .delete(&roots, &id, delete_file.unwrap_or(true))
        .map_err(|error| ipc(&error, "delete_local_music"))
}

/// Single local track, so the frontend can queue it by id.
#[tauri::command]
pub fn get_local_track(state: State<'_, AppState>, id: String) -> Result<LocalTrack, IpcError> {
    service(&state)
        .get(&id)
        .map_err(|error| ipc(&error, "get_local_track"))?
        .ok_or_else(|| {
            IpcError::new(
                "LOCAL_TRACK_NOT_FOUND",
                "本地曲目不存在",
                false,
                "get_local_track".to_owned(),
            )
        })
}

/// One page of duplicate groups.
///
/// Grouping runs over the whole library, then the groups are paged: paging the
/// tracks first would split a group across pages.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroupsPage {
    pub groups: Vec<Vec<LocalTrack>>,
    pub page: u32,
    pub page_size: u32,
    /// Number of groups, not of tracks.
    pub total: u64,
    pub total_pages: u32,
}

/// Tracks that look like the same recording, grouped and paged.
#[tauri::command]
pub fn get_local_duplicates(
    state: State<'_, AppState>,
    page: u32,
    page_size: u32,
) -> Result<DuplicateGroupsPage, IpcError> {
    let page = page.max(1);
    let page_size = if page_size == 0 {
        DUPLICATES_PAGE_SIZE
    } else {
        page_size.min(MAX_DUPLICATES_PAGE_SIZE)
    };

    let roots = roots(&state);
    let tracks = service(&state)
        .all_tracks(&roots)
        .map_err(|error| ipc(&error, "get_local_duplicates"))?;
    let groups = duplicate_groups(&tracks);

    let total = groups.len() as u64;
    let offset = usize::try_from(u64::from(page - 1) * u64::from(page_size)).unwrap_or(usize::MAX);
    let groups = groups
        .into_iter()
        .skip(offset)
        .take(page_size as usize)
        .collect();

    Ok(DuplicateGroupsPage {
        groups,
        page,
        page_size,
        total,
        total_pages: total_pages(total, page_size),
    })
}

/// How many queued songs the library already has.
///
/// Lets the UI warn before a batch download starts, instead of reporting the
/// skips only once every task has run.
#[tauri::command]
pub fn precheck_downloads(
    state: State<'_, AppState>,
    songs: Vec<PrecheckSong>,
) -> Result<PrecheckReport, IpcError> {
    validate_precheck(&songs).map_err(|message| {
        tracing::warn!(event = "precheck_rejected", count = songs.len(), %message);
        IpcError::new(
            "INVALID_ARGUMENT",
            &message,
            false,
            "precheck_downloads".to_owned(),
        )
    })?;

    let roots = roots(&state);
    let library = service(&state)
        .all_tracks(&roots)
        .map_err(|error| ipc(&error, "precheck_downloads"))?;
    Ok(precheck(&songs, &library))
}

fn service(state: &State<'_, AppState>) -> LocalMusicService {
    LocalMusicService::new(Arc::clone(&state.database))
}

/// Music directories from the current settings; empty when none exist yet.
fn roots(state: &State<'_, AppState>) -> Vec<PathBuf> {
    music_roots(&state.paths, &state.settings_snapshot())
}

fn ipc(error: &LocalMusicError, operation: &str) -> IpcError {
    tracing::warn!(event = "local_music_command_failed", operation, %error);
    IpcError::new(
        error.code(),
        &error.to_string(),
        error.retryable(),
        operation.to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::RefreshReport;

    #[test]
    fn refresh_report_uses_the_camel_case_ipc_contract() {
        let value = serde_json::to_value(RefreshReport {
            seen: 4,
            added: 2,
            updated: 1,
            removed: 1,
        })
        .unwrap_or_default();
        assert_eq!(value["seen"], 4);
        assert_eq!(value["added"], 2);
        assert_eq!(value["updated"], 1);
        assert_eq!(value["removed"], 1);
    }
}
