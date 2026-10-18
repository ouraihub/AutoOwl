use serde::Serialize;

/// ffmpeg / ffprobe availability, as rendered on the media settings page.
///
/// A missing tool is a state rather than an error, so the reason travels
/// alongside the flag: the page can say *why* a tool is unusable (not on PATH,
/// override points at a directory, binary will not execute) instead of only
/// "unavailable".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaToolStatus {
    pub ffmpeg: bool,
    pub ffprobe: bool,
    pub ffmpeg_path: Option<String>,
    pub ffprobe_path: Option<String>,
    /// Chinese explanation, present only when `ffmpeg` is false.
    pub ffmpeg_error: Option<String>,
    pub ffprobe_error: Option<String>,
}
