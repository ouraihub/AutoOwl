//! "My collections" commands.
//!
//! Backs the local playlist UI: create/rename/delete, add/remove songs,
//! reorder, and import an external playlist as a snapshot.

use std::sync::Arc;

use serde::Deserialize;
use tauri::State;

use crate::application::collection::parse_content_type;
use crate::application::collection_use_case::CollectionService;
use crate::domain::source::Song;
use crate::error::IpcError;
use crate::infrastructure::collection_repository::{
    AddOutcome, CollectionStoreError, ImportMetadata, StoredCollection,
};
use crate::state::AppState;

/// One `(song_id, source)` pair in a requested order.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongKey {
    pub song_id: String,
    pub source: String,
}

#[tauri::command]
pub fn list_collections(state: State<'_, AppState>) -> Result<Vec<StoredCollection>, IpcError> {
    service(&state)
        .list()
        .map_err(|error| ipc(&error, "list_collections"))
}

#[tauri::command]
pub fn create_collection(
    state: State<'_, AppState>,
    name: String,
    description: Option<String>,
) -> Result<StoredCollection, IpcError> {
    service(&state)
        .create(&name, description.as_deref())
        .map_err(|error| ipc(&error, "create_collection"))
}

#[tauri::command]
pub fn rename_collection(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<StoredCollection, IpcError> {
    service(&state)
        .rename(&id, &name)
        .map_err(|error| ipc(&error, "rename_collection"))
}

#[tauri::command]
pub fn delete_collection(state: State<'_, AppState>, id: String) -> Result<(), IpcError> {
    service(&state)
        .delete(&id)
        .map_err(|error| ipc(&error, "delete_collection"))
}

#[tauri::command]
pub fn get_collection_songs(state: State<'_, AppState>, id: String) -> Result<Vec<Song>, IpcError> {
    service(&state)
        .songs(&id)
        .map_err(|error| ipc(&error, "get_collection_songs"))
}

/// Adds one or more songs; already-saved songs are reported as `skipped`.
#[tauri::command]
pub fn add_collection_songs(
    state: State<'_, AppState>,
    id: String,
    songs: Vec<Song>,
) -> Result<AddOutcome, IpcError> {
    service(&state)
        .add_songs(&id, &songs)
        .map_err(|error| ipc(&error, "add_collection_songs"))
}

#[tauri::command]
pub fn remove_collection_song(
    state: State<'_, AppState>,
    id: String,
    song_id: String,
    source: String,
) -> Result<(), IpcError> {
    service(&state)
        .remove_song(&id, &song_id, &source)
        .map_err(|error| ipc(&error, "remove_collection_song"))
}

/// Rewrites the stored order; unlisted songs keep their relative order after
/// the listed ones.
#[tauri::command]
pub fn reorder_collection_songs(
    state: State<'_, AppState>,
    id: String,
    order: Vec<SongKey>,
) -> Result<(), IpcError> {
    let keys: Vec<(String, String)> = order
        .into_iter()
        .map(|key| (key.song_id, key.source))
        .collect();
    service(&state)
        .reorder(&id, &keys)
        .map_err(|error| ipc(&error, "reorder_collection_songs"))
}

/// Imports an external playlist as an immutable snapshot.
#[tauri::command]
pub fn import_collection(
    state: State<'_, AppState>,
    name: String,
    songs: Vec<Song>,
    metadata: Option<ImportMetadata>,
    content_type: Option<String>,
) -> Result<StoredCollection, IpcError> {
    let content_type =
        parse_content_type(content_type.as_deref().unwrap_or_default()).map_err(|message| {
            IpcError::new(
                "INVALID_ARGUMENT",
                &message,
                false,
                "import_collection".to_owned(),
            )
        })?;
    service(&state)
        .import(&name, &songs, &metadata.unwrap_or_default(), content_type)
        .map_err(|error| ipc(&error, "import_collection"))
}

fn service(state: &State<'_, AppState>) -> CollectionService {
    CollectionService::new(Arc::clone(&state.database))
}

fn ipc(error: &CollectionStoreError, operation: &str) -> IpcError {
    tracing::warn!(event = "collection_command_failed", operation, %error);
    IpcError::new(
        error.code(),
        &error.message(),
        error.retryable(),
        operation.to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::SongKey;

    #[test]
    fn song_key_deserializes_from_the_camel_case_contract() {
        let key: SongKey =
            serde_json::from_str(r#"{"songId":"42","source":"netease"}"#).expect("deserialize");
        assert_eq!(key.song_id, "42");
        assert_eq!(key.source, "netease");
    }
}
