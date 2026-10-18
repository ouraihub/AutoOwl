//! Cover + audio → mp4 rendering via ffmpeg.
//!
//! ffmpeg is required here and cannot be worked around in-process: there is no
//! pure-Rust muxer in the dependency set. When it is missing the render is
//! refused with `MEDIA_TOOL_MISSING` rather than reported as queued forever, so
//! the UI can tell the user to install it.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::application::tasks::{TaskRegistry, TaskState};
use crate::infrastructure::media_tool::ResolveFailure;

/// Error code the frontend switches on to show the install hint.
pub const MEDIA_TOOL_MISSING: &str = "MEDIA_TOOL_MISSING";
pub const RENDER_FAILED: &str = "RENDER_FAILED";
pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderState {
    Queued,
    Preparing,
    Rendering,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderTask {
    pub id: String,
    pub state: RenderState,
    pub progress: f32,
    pub output_path: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    ToolMissing(ResolveFailure),
    #[error("{0}")]
    Failed(String),
}

impl RenderError {
    /// Stable code for the IPC envelope.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => INVALID_ARGUMENT,
            Self::ToolMissing(_) => MEDIA_TOOL_MISSING,
            Self::Failed(_) => RENDER_FAILED,
        }
    }

    /// Whether retrying unchanged could succeed. A missing binary cannot.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

pub fn validate_render_input(song_id: &str, source: &str) -> Result<(), String> {
    if song_id.trim().is_empty() || source.trim().is_empty() {
        return Err("渲染输入不完整".to_owned());
    }
    Ok(())
}

/// Inputs for one render.
#[derive(Debug, Clone)]
pub struct RenderRequest {
    pub song_id: String,
    pub source: String,
    /// Still image used as the video track.
    pub cover_path: PathBuf,
    pub audio_path: PathBuf,
    /// Directory for the mp4, normally `AppPaths::video_output`.
    pub output_dir: PathBuf,
}

impl RenderRequest {
    /// Output file name, unique per source + song.
    pub fn output_path(&self) -> PathBuf {
        let stem = sanitize_stem(&format!("{}-{}", self.source, self.song_id));
        self.output_dir.join(format!("{stem}.mp4"))
    }

    fn validate(&self) -> Result<(), RenderError> {
        validate_render_input(&self.song_id, &self.source).map_err(RenderError::InvalidInput)?;
        if !self.audio_path.is_file() {
            return Err(RenderError::InvalidInput(format!(
                "音频文件不存在：{}",
                self.audio_path.display()
            )));
        }
        if !self.cover_path.is_file() {
            return Err(RenderError::InvalidInput(format!(
                "封面文件不存在：{}",
                self.cover_path.display()
            )));
        }
        Ok(())
    }
}

/// Keeps a task id usable as a file name.
fn sanitize_stem(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "render".to_owned()
    } else {
        cleaned
    }
}

/// ffmpeg arguments for a still-image video muxed with the source audio.
///
/// The audio is stream-copied, so no re-encode and no quality loss; only the
/// cover is encoded, as a single looped frame. `-shortest` ends the video with
/// the audio, and `+faststart` puts the moov atom first so the file plays while
/// still downloading.
pub fn ffmpeg_args(cover: &Path, audio: &Path, output: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    for flag in ["-y", "-hide_banner", "-loglevel", "error"] {
        args.push(flag.into());
    }
    args.push("-loop".into());
    args.push("1".into());
    args.push("-i".into());
    args.push(cover.into());
    args.push("-i".into());
    args.push(audio.into());
    for flag in [
        "-c:v",
        "libx264",
        "-tune",
        "stillimage",
        "-pix_fmt",
        "yuv420p",
        // Even dimensions are required by yuv420p; covers are often odd-sized.
        "-vf",
        "scale=trunc(iw/2)*2:trunc(ih/2)*2",
        "-c:a",
        "copy",
        "-shortest",
        "-movflags",
        "+faststart",
    ] {
        args.push(flag.into());
    }
    args.push(output.into());
    args
}

/// Renders `request` to mp4, reporting progress through `tasks`.
///
/// `ffmpeg` is the resolved binary path; callers obtain it from
/// `media_tool::resolve_ffmpeg` and map its failure to
/// `RenderError::ToolMissing` so the refusal happens before any task state is
/// published.
pub async fn render_video(
    task_id: &str,
    ffmpeg: &Path,
    request: &RenderRequest,
    tasks: &TaskRegistry,
) -> Result<PathBuf, RenderError> {
    tasks.set(task_id.to_owned(), TaskState::Resolving);
    if let Err(error) = request.validate() {
        tasks.set(task_id.to_owned(), TaskState::Failed);
        return Err(error);
    }

    tokio::fs::create_dir_all(&request.output_dir)
        .await
        .map_err(|error| {
            tasks.set(task_id.to_owned(), TaskState::Failed);
            RenderError::Failed(format!("无法创建输出目录：{error}"))
        })?;

    let output = request.output_path();
    tasks.set(task_id.to_owned(), TaskState::Processing);

    let mut command = tokio::process::Command::new(ffmpeg);
    command
        .args(ffmpeg_args(
            &request.cover_path,
            &request.audio_path,
            &output,
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    // Keeps a console window from flashing during a render.
    command.creation_flags(0x0800_0000);

    let result = command
        .output()
        .await
        .map_err(|error| RenderError::Failed(format!("ffmpeg 启动失败：{error}")));
    let result = match result {
        Ok(output) => output,
        Err(error) => {
            tasks.set(task_id.to_owned(), TaskState::Failed);
            return Err(error);
        }
    };

    if !result.status.success() {
        tasks.set(task_id.to_owned(), TaskState::Failed);
        let stderr = String::from_utf8_lossy(&result.stderr);
        let detail = stderr.lines().last().unwrap_or("无输出").trim().to_owned();
        // Remove the partial file so a failed render cannot be mistaken for a
        // finished one by anything scanning the output directory.
        let _ = tokio::fs::remove_file(&output).await;
        return Err(RenderError::Failed(format!("视频渲染失败：{detail}")));
    }

    let produced = tokio::fs::metadata(&output)
        .await
        .map(|meta| meta.len() > 0)
        .unwrap_or(false);
    if !produced {
        tasks.set(task_id.to_owned(), TaskState::Failed);
        return Err(RenderError::Failed("视频渲染结果为空".to_owned()));
    }

    tasks.set(task_id.to_owned(), TaskState::Completed);
    Ok(output)
}

/// Task snapshot for a refused render.
pub fn failed_task(id: String, error: &RenderError) -> RenderTask {
    RenderTask {
        id,
        state: RenderState::Failed,
        progress: 0.0,
        output_path: None,
        error_code: Some(error.code().to_owned()),
    }
}

/// Task snapshot for a finished render.
pub fn completed_task(id: String, output: &Path) -> RenderTask {
    RenderTask {
        id,
        state: RenderState::Completed,
        progress: 1.0,
        output_path: Some(output.display().to_string()),
        error_code: None,
    }
}
