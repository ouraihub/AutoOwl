//! Data/logs locations for the settings page.
//!
//! The running process keeps the locations it started with; a change is written
//! to `autoowl.paths.json` and applies after a restart (see `crate::storage`).

use std::path::Path;

use serde::Serialize;
use tauri::State;
use tauri_plugin_dialog::DialogExt;

use crate::error::IpcError;
use crate::state::AppState;
use crate::storage::{self, PathOverrides};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoragePaths {
    /// In use by this process.
    pub data_dir: String,
    pub logs_dir: String,
    /// What `autoowl.paths.json` asks for; empty means "default".
    pub data_dir_override: String,
    pub logs_dir_override: String,
    /// Defaults when there is no override.
    pub default_data_dir: String,
    pub default_logs_dir: String,
    /// The file differs from what is in use: a restart applies it.
    pub restart_required: bool,
}

#[tauri::command]
pub fn get_storage_paths(state: State<'_, AppState>) -> Result<StoragePaths, IpcError> {
    let exe_dir = storage::exe_dir().map_err(storage_error)?;
    let overrides = storage::read_overrides(&exe_dir).map_err(storage_error)?;
    Ok(report(
        &exe_dir,
        &overrides,
        &state.paths.root,
        &state.paths.logs,
    ))
}

/// Validates and saves new locations; an empty string restores the default.
/// Each chosen directory must be absolute and actually writable, so a bad
/// choice is reported now instead of blocking the next launch.
#[tauri::command]
pub fn set_storage_paths(
    state: State<'_, AppState>,
    data_dir: String,
    logs_dir: String,
) -> Result<StoragePaths, IpcError> {
    let exe_dir = storage::exe_dir().map_err(storage_error)?;
    let overrides = PathOverrides {
        data_dir: Some(data_dir),
        logs_dir: Some(logs_dir),
    };
    let wanted = storage::locations(&exe_dir, &overrides);
    for (dir, label) in [(&wanted.data, "数据目录"), (&wanted.logs, "日志目录")] {
        if !dir.is_absolute() {
            return Err(storage_error(format!(
                "{label}必须是完整路径：{}",
                dir.display()
            )));
        }
        storage::ensure_writable(dir, label).map_err(storage_error)?;
    }
    storage::write_overrides(&exe_dir, &overrides).map_err(storage_error)?;
    tracing::info!(
        event = "storage_paths_saved",
        data = %wanted.data.display(),
        logs = %wanted.logs.display()
    );
    let saved = storage::read_overrides(&exe_dir).map_err(storage_error)?;
    Ok(report(
        &exe_dir,
        &saved,
        &state.paths.root,
        &state.paths.logs,
    ))
}

/// Native folder picker; `None` when the user cancels.
///
/// Async so the blocking dialog runs off the main thread (a sync command runs
/// on it, and the dialog would deadlock waiting for the event loop).
#[tauri::command]
pub async fn pick_folder(app: tauri::AppHandle, start: Option<String>) -> Option<String> {
    let mut dialog = app.dialog().file().set_title("选择文件夹");
    if let Some(start) = start.filter(|dir| Path::new(dir).is_dir()) {
        dialog = dialog.set_directory(start);
    }
    dialog
        .blocking_pick_folder()
        .and_then(|folder| folder.into_path().ok())
        .map(|path| path.display().to_string())
}

/// Restarts the app so new locations take effect.
#[tauri::command]
pub fn restart_app(app: tauri::AppHandle) {
    tracing::info!(event = "app_restart_requested");
    app.restart();
}

fn report(exe_dir: &Path, overrides: &PathOverrides, data: &Path, logs: &Path) -> StoragePaths {
    let defaults = storage::locations(exe_dir, &PathOverrides::default());
    let wanted = storage::locations(exe_dir, overrides);
    StoragePaths {
        data_dir: data.display().to_string(),
        logs_dir: logs.display().to_string(),
        data_dir_override: overrides.data_dir.clone().unwrap_or_default(),
        logs_dir_override: overrides.logs_dir.clone().unwrap_or_default(),
        default_data_dir: defaults.data.display().to_string(),
        default_logs_dir: defaults.logs.display().to_string(),
        restart_required: wanted.data != data || wanted.logs != logs,
    }
}

fn storage_error(message: String) -> IpcError {
    IpcError::new(
        "STORAGE_PATH_INVALID",
        &message,
        false,
        "storage".to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_is_required_only_when_the_file_differs_from_what_is_in_use() {
        let exe = Path::new(r"D:\Apps\AutoOwl");
        let (data, logs) = (exe.join("data"), exe.join("data").join("logs"));

        let current = report(exe, &PathOverrides::default(), &data, &logs);
        assert!(!current.restart_required);
        assert_eq!(current.default_data_dir, data.display().to_string());

        let moved = PathOverrides {
            data_dir: Some(r"E:\OwlData".into()),
            logs_dir: None,
        };
        let pending = report(exe, &moved, &data, &logs);
        // Logs follow the data dir by default, so both move.
        assert!(pending.restart_required);
        assert_eq!(pending.data_dir_override, r"E:\OwlData");
        assert!(pending.logs_dir_override.is_empty());
    }
}
