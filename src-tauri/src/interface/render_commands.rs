use std::path::PathBuf;

use tauri::State;

use crate::application::render::{
    completed_task, render_video, RenderError, RenderRequest, RenderTask,
};
use crate::error::IpcError;
use crate::infrastructure::media_tool::resolve_ffmpeg;
use crate::state::AppState;

const OPERATION: &str = "start_video_render";

fn to_ipc(error: &RenderError) -> IpcError {
    IpcError::new(
        error.code(),
        &error.to_string(),
        error.retryable(),
        OPERATION.to_owned(),
    )
}

/// Renders a cover + audio pair into an mp4 under `AppPaths::video_output`.
///
/// Runs to completion before answering: the render is a single ffmpeg
/// invocation with no intermediate state worth streaming, and the task registry
/// carries progress for anything polling meanwhile.
#[tauri::command]
pub async fn start_video_render(
    state: State<'_, AppState>,
    song_id: String,
    source: String,
    cover_path: String,
    audio_path: String,
) -> Result<RenderTask, IpcError> {
    let task_id = state.next_task_id("render");

    // Resolved before any work so a machine without ffmpeg gets
    // MEDIA_TOOL_MISSING immediately instead of a task that cannot progress.
    let ffmpeg = match resolve_ffmpeg() {
        Ok(path) => path,
        Err(failure) => {
            let error = RenderError::ToolMissing(failure);
            return Err(to_ipc(&error));
        }
    };

    let request = RenderRequest {
        song_id,
        source,
        cover_path: PathBuf::from(cover_path),
        audio_path: PathBuf::from(audio_path),
        output_dir: state.paths.video_output.clone(),
    };

    match render_video(&task_id, &ffmpeg, &request, &state.task_registry).await {
        Ok(output) => Ok(completed_task(task_id, &output)),
        Err(error) => Err(to_ipc(&error)),
    }
}
