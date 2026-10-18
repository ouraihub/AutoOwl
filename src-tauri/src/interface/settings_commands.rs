use crate::domain::media_status::MediaToolStatus;
use crate::domain::settings::AppSettings;
use crate::error::IpcError;
use crate::infrastructure::media_tool::{MediaToolPaths, ResolutionContext};
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, IpcError> {
    state
        .settings
        .lock()
        .map(|settings| settings.clone())
        .map_err(|_| state_error())
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, IpcError> {
    let normalized = settings.normalize();
    *state.settings.lock().map_err(|_| state_error())? = normalized.clone();
    Ok(normalized)
}

fn state_error() -> IpcError {
    IpcError::new(
        "STATE_UNAVAILABLE",
        "设置暂时不可用",
        true,
        "settings".to_owned(),
    )
}

/// Detects ffmpeg/ffprobe on demand.
///
/// Resolution executes each candidate, so this runs off the UI thread; the
/// settings page calls it on mount and after the user installs the tools.
#[tauri::command]
pub async fn get_media_tool_status() -> MediaToolStatus {
    tauri::async_runtime::spawn_blocking(|| {
        MediaToolPaths::resolve(&ResolutionContext::system()).to_status()
    })
    .await
    .unwrap_or_else(|_| MediaToolStatus {
        ffmpeg: false,
        ffprobe: false,
        ffmpeg_path: None,
        ffprobe_path: None,
        ffmpeg_error: Some("媒体工具检测失败".to_owned()),
        ffprobe_error: Some("媒体工具检测失败".to_owned()),
    })
}

/// Installs ffmpeg with Scoop, then reports the fresh detection result.
/// Takes minutes on a slow link; the page shows progress until it resolves.
#[tauri::command]
pub async fn install_media_tools() -> Result<MediaToolStatus, IpcError> {
    crate::infrastructure::media_tool::install_with_scoop()
        .await
        .map_err(|message| IpcError::new("INSTALL_FAILED", &message, true, "media".to_owned()))?;
    Ok(get_media_tool_status().await)
}
