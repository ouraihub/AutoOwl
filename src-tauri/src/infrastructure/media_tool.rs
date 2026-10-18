//! ffmpeg / ffprobe discovery and media probing.
//!
//! Lookup order: explicit env override, then a binary bundled next to the app,
//! then `PATH`.
//!
//! A candidate is accepted only once it answers `-version`. Existence checks
//! alone accept a directory named `ffmpeg`, a zero-byte placeholder or a stub
//! left behind by an uninstall, and the failure would then surface much later
//! as an unexplained render error.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::media_status::MediaToolStatus;

/// Explicit path overrides; an unset or blank value falls through to the next step.
pub const FFMPEG_ENV_VAR: &str = "AUTOOWL_FFMPEG";
pub const FFPROBE_ENV_VAR: &str = "AUTOOWL_FFPROBE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaToolKind {
    Ffmpeg,
    Ffprobe,
}

impl MediaToolKind {
    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
        }
    }

    pub fn env_var(self) -> &'static str {
        match self {
            Self::Ffmpeg => FFMPEG_ENV_VAR,
            Self::Ffprobe => FFPROBE_ENV_VAR,
        }
    }
}

/// Why a tool could not be resolved. Rendered straight into the settings page,
/// so the text is Chinese and always names the offending path.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveFailure {
    #[error("{variable} 指向的文件不存在：{path}")]
    Missing { variable: String, path: String },
    #[error("{variable} 指向的是目录，不是可执行文件：{path}")]
    Directory { variable: String, path: String },
    #[error("{variable} 指向的文件无法执行：{path}")]
    NotExecutable { variable: String, path: String },
    #[error("未找到 {binary}，请安装后加入 PATH，或用 {variable} 指定可执行文件路径")]
    NotFound {
        binary: &'static str,
        variable: &'static str,
    },
}

impl ResolveFailure {
    fn not_found(kind: MediaToolKind) -> Self {
        Self::NotFound {
            binary: kind.binary_name(),
            variable: kind.env_var(),
        }
    }
}

/// Injected lookups, so resolution is testable without mutating process
/// environment or requiring a real ffmpeg on the machine.
pub struct ResolutionContext<'a> {
    /// Environment reader. `PATH` is read through here as well.
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// Directory holding a bundled binary, normally the executable's directory.
    pub bundled_dir: Option<PathBuf>,
    /// Candidate acceptance test; production runs `<candidate> -version`.
    pub verify: &'a dyn Fn(&Path) -> bool,
}

impl ResolutionContext<'_> {
    /// Context used by the running application.
    pub fn system() -> ResolutionContext<'static> {
        ResolutionContext {
            env: &system_env,
            bundled_dir: bundled_dir(),
            verify: &answers_version,
        }
    }
}

fn system_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Directory the running executable lives in; bundled sidecars sit beside it.
fn bundled_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// Executable name variants to try for a bare tool name.
fn name_variants(binary: &str) -> Vec<String> {
    if cfg!(windows) {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|extension| format!("{binary}.{extension}"))
            .chain(std::iter::once(binary.to_owned()))
            .collect()
    } else {
        vec![binary.to_owned()]
    }
}

/// Runs `<path> -version`, the cheapest call both tools answer.
fn answers_version(path: &Path) -> bool {
    if path.is_dir() {
        return false;
    }
    let mut command = std::process::Command::new(path);
    command
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut command);
    matches!(command.status(), Ok(status) if status.success())
}

/// Keeps a console window from flashing on Windows.
#[cfg(windows)]
fn hide_console(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut std::process::Command) {}

#[cfg(windows)]
fn hide_console_async(command: &mut tokio::process::Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_async(_command: &mut tokio::process::Command) {}

/// Searches `PATH` for an executable named `binary`.
fn search_path(binary: &str, ctx: &ResolutionContext<'_>) -> Option<PathBuf> {
    let path_var = (ctx.env)("PATH")?;
    let variants = name_variants(binary);
    std::env::split_paths(&path_var)
        .filter(|directory| !directory.as_os_str().is_empty())
        .flat_map(|directory| {
            variants
                .iter()
                .map(move |variant| directory.join(variant))
                .collect::<Vec<_>>()
        })
        .find(|candidate| (ctx.verify)(candidate))
}

/// Validates an explicitly configured override.
///
/// A broken override is an error rather than a silent fall-through to `PATH`:
/// the user asked for a specific binary and needs to hear that it is wrong.
fn resolve_override(
    variable: &str,
    value: &str,
    kind: MediaToolKind,
    ctx: &ResolutionContext<'_>,
) -> Result<PathBuf, ResolveFailure> {
    let candidate = Path::new(value);
    let is_bare_name = candidate
        .parent()
        .map(Path::as_os_str)
        .is_none_or(std::ffi::OsStr::is_empty);
    if is_bare_name {
        return search_path(value, ctx).ok_or_else(|| ResolveFailure::Missing {
            variable: variable.to_owned(),
            path: value.to_owned(),
        });
    }
    if candidate.is_dir() {
        return Err(ResolveFailure::Directory {
            variable: variable.to_owned(),
            path: value.to_owned(),
        });
    }
    if !candidate.exists() {
        return Err(ResolveFailure::Missing {
            variable: variable.to_owned(),
            path: value.to_owned(),
        });
    }
    if !(ctx.verify)(candidate) {
        return Err(ResolveFailure::NotExecutable {
            variable: variable.to_owned(),
            path: value.to_owned(),
        });
    }
    let _ = kind;
    Ok(candidate.to_path_buf())
}

/// Resolves one tool: env override, then a bundled binary, then `PATH`.
pub fn resolve(
    kind: MediaToolKind,
    ctx: &ResolutionContext<'_>,
) -> Result<PathBuf, ResolveFailure> {
    let variable = kind.env_var();
    if let Some(value) = (ctx.env)(variable) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return resolve_override(variable, trimmed, kind, ctx);
        }
    }

    if let Some(directory) = ctx.bundled_dir.as_ref() {
        let bundled = name_variants(kind.binary_name())
            .into_iter()
            .map(|variant| directory.join(variant))
            .find(|candidate| (ctx.verify)(candidate));
        if let Some(found) = bundled {
            return Ok(found);
        }
    }

    search_path(kind.binary_name(), ctx)
        .or_else(|| search_scoop_shims(kind.binary_name(), ctx))
        .ok_or_else(|| ResolveFailure::not_found(kind))
}

/// Scoop's shims directory: `%SCOOP%\shims`, else `%USERPROFILE%\scoop\shims`.
fn scoop_shims(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("SCOOP")
        .filter(|root| !root.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| env("USERPROFILE").map(|home| PathBuf::from(home).join("scoop")))
        .map(|root| root.join("shims"))
}

/// After `scoop install`, the shims are on the user's PATH but not on this
/// process's (inherited at launch), so look there directly instead of
/// requiring a restart.
fn search_scoop_shims(binary: &str, ctx: &ResolutionContext<'_>) -> Option<PathBuf> {
    let shims = scoop_shims(ctx.env)?;
    name_variants(binary)
        .into_iter()
        .map(|variant| shims.join(variant))
        .find(|candidate| (ctx.verify)(candidate))
}

/// Installs ffmpeg (which ships ffprobe) with Scoop, without a console window.
///
/// Scoop installs per user and needs no admin rights. Errors are Chinese and
/// shown as-is: "Scoop not installed" is the common case, and the page then
/// points to manual install instead.
pub async fn install_with_scoop() -> Result<(), String> {
    let scoop = scoop_shims(&system_env)
        .map(|shims| shims.join("scoop.cmd"))
        .filter(|cmd| cmd.is_file())
        .ok_or_else(|| "未检测到 Scoop，请先安装 Scoop（scoop.sh）或手动安装 ffmpeg".to_owned())?;
    let mut command = tokio::process::Command::new(scoop);
    command
        .args(["install", "ffmpeg"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console_async(&mut command);
    let output = command
        .output()
        .await
        .map_err(|error| format!("Scoop 启动失败：{error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let log = String::from_utf8_lossy(&output.stderr);
    let last = log
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("未知错误");
    Err(format!("Scoop 安装 ffmpeg 失败：{}", last.trim()))
}

/// Resolution outcome for both tools.
pub struct MediaToolPaths {
    pub ffmpeg: Result<PathBuf, ResolveFailure>,
    pub ffprobe: Result<PathBuf, ResolveFailure>,
}

impl MediaToolPaths {
    pub fn resolve(ctx: &ResolutionContext<'_>) -> Self {
        Self {
            ffmpeg: resolve(MediaToolKind::Ffmpeg, ctx),
            ffprobe: resolve(MediaToolKind::Ffprobe, ctx),
        }
    }

    /// Flattens into the shape the settings page renders.
    ///
    /// A missing tool is a state, not a failure: the UI shows "unavailable"
    /// with the reason and every feature that needs it degrades on its own.
    pub fn to_status(&self) -> MediaToolStatus {
        MediaToolStatus {
            ffmpeg: self.ffmpeg.is_ok(),
            ffprobe: self.ffprobe.is_ok(),
            ffmpeg_path: path_string(&self.ffmpeg),
            ffprobe_path: path_string(&self.ffprobe),
            ffmpeg_error: failure_string(&self.ffmpeg),
            ffprobe_error: failure_string(&self.ffprobe),
        }
    }
}

fn path_string(resolved: &Result<PathBuf, ResolveFailure>) -> Option<String> {
    resolved
        .as_ref()
        .ok()
        .map(|path| path.display().to_string())
}

fn failure_string(resolved: &Result<PathBuf, ResolveFailure>) -> Option<String> {
    resolved.as_ref().err().map(ToString::to_string)
}

/// Resolves ffmpeg against the real environment.
pub fn resolve_ffmpeg() -> Result<PathBuf, ResolveFailure> {
    resolve(MediaToolKind::Ffmpeg, &ResolutionContext::system())
}

/// Resolves ffprobe against the real environment.
pub fn resolve_ffprobe() -> Result<PathBuf, ResolveFailure> {
    resolve(MediaToolKind::Ffprobe, &ResolutionContext::system())
}

/// Media facts read back from a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    /// Bits per second, as reported by ffprobe.
    pub bitrate: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum MediaToolError {
    #[error("{0}")]
    NotFound(ResolveFailure),
    #[error("媒体信息读取失败：{0}")]
    Failed(String),
}

#[async_trait]
pub trait MediaTool: Send + Sync {
    /// Reads title/artist/album/duration/bitrate from a media file.
    async fn probe(&self, path: &str) -> Result<MediaMetadata, MediaToolError>;

    /// Which tools are available, and where they were found.
    fn status(&self) -> MediaToolStatus;
}

/// ffprobe's `-show_format -show_streams` payload, only the fields used here.
#[derive(Debug, Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    format: Option<ProbeFormat>,
    #[serde(default)]
    streams: Vec<ProbeStream>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    /// Seconds, as a decimal string.
    #[serde(default)]
    duration: Option<String>,
    /// Bits per second, as a decimal string.
    #[serde(default)]
    bit_rate: Option<String>,
    #[serde(default)]
    tags: Option<ProbeTags>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    #[serde(default)]
    codec_type: Option<String>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    bit_rate: Option<String>,
    #[serde(default)]
    tags: Option<ProbeTags>,
}

/// Tag keys vary in case across containers, so every lookup is
/// case-insensitive rather than relying on serde field names.
#[derive(Debug, Deserialize)]
struct ProbeTags(std::collections::HashMap<String, serde_json::Value>);

impl ProbeTags {
    fn get(&self, key: &str) -> Option<String> {
        self.0
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .and_then(|(_, value)| value.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    }
}

/// Seconds as a decimal string to whole milliseconds.
fn seconds_to_millis(raw: &str) -> Option<u64> {
    let seconds = raw.trim().parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    Some((seconds * 1000.0).round() as u64)
}

fn parse_bitrate(raw: &str) -> Option<u64> {
    let value = raw.trim().parse::<u64>().ok()?;
    (value > 0).then_some(value)
}

/// Folds an ffprobe JSON document into `MediaMetadata`.
///
/// Container-level values win; the first audio stream fills whatever the
/// container left out, which is how m4a and flac report bitrate.
pub fn parse_probe_output(json: &str) -> Result<MediaMetadata, MediaToolError> {
    let parsed: ProbeOutput = serde_json::from_str(json)
        .map_err(|error| MediaToolError::Failed(format!("ffprobe 输出解析失败：{error}")))?;
    let mut metadata = MediaMetadata::default();

    if let Some(format) = parsed.format.as_ref() {
        if let Some(tags) = format.tags.as_ref() {
            metadata.title = tags.get("title");
            metadata.artist = tags.get("artist");
            metadata.album = tags.get("album");
        }
        metadata.duration_ms = format.duration.as_deref().and_then(seconds_to_millis);
        metadata.bitrate = format.bit_rate.as_deref().and_then(parse_bitrate);
    }

    let audio = parsed
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("audio"));
    if let Some(stream) = audio {
        if let Some(tags) = stream.tags.as_ref() {
            metadata.title = metadata.title.or_else(|| tags.get("title"));
            metadata.artist = metadata.artist.or_else(|| tags.get("artist"));
            metadata.album = metadata.album.or_else(|| tags.get("album"));
        }
        metadata.duration_ms = metadata
            .duration_ms
            .or_else(|| stream.duration.as_deref().and_then(seconds_to_millis));
        metadata.bitrate = metadata
            .bitrate
            .or_else(|| stream.bit_rate.as_deref().and_then(parse_bitrate));
    }

    Ok(metadata)
}

/// Probes via a resolved `ffprobe` binary.
pub struct FfmpegMediaTool {
    ffmpeg: Result<PathBuf, ResolveFailure>,
    ffprobe: Result<PathBuf, ResolveFailure>,
}

impl FfmpegMediaTool {
    /// Resolves both tools once, against the real environment.
    ///
    /// Resolution executes candidates, so it is deliberately not repeated per
    /// call; the settings page has a refresh path if the user installs ffmpeg
    /// while the app is running.
    pub fn detect() -> Self {
        Self::from_paths(MediaToolPaths::resolve(&ResolutionContext::system()))
    }

    pub fn from_paths(paths: MediaToolPaths) -> Self {
        Self {
            ffmpeg: paths.ffmpeg,
            ffprobe: paths.ffprobe,
        }
    }

    /// Path to ffmpeg, or the reason it is unusable.
    pub fn ffmpeg_path(&self) -> Result<&Path, &ResolveFailure> {
        self.ffmpeg.as_deref()
    }

    pub fn ffprobe_path(&self) -> Result<&Path, &ResolveFailure> {
        self.ffprobe.as_deref()
    }

    pub fn has_ffmpeg(&self) -> bool {
        self.ffmpeg.is_ok()
    }
}

#[async_trait]
impl MediaTool for FfmpegMediaTool {
    async fn probe(&self, path: &str) -> Result<MediaMetadata, MediaToolError> {
        let ffprobe = self
            .ffprobe
            .as_ref()
            .map_err(|failure| MediaToolError::NotFound(failure.clone()))?;
        let mut command = tokio::process::Command::new(ffprobe);
        command
            .args([
                "-v",
                "quiet",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
            ])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_console_async(&mut command);
        let output = command
            .output()
            .await
            .map_err(|error| MediaToolError::Failed(format!("ffprobe 启动失败：{error}")))?;
        if !output.status.success() {
            return Err(MediaToolError::Failed(format!(
                "ffprobe 退出码 {}",
                output
                    .status
                    .code()
                    .map_or_else(|| "未知".to_owned(), |code| code.to_string())
            )));
        }
        let json = String::from_utf8_lossy(&output.stdout);
        parse_probe_output(&json)
    }

    fn status(&self) -> MediaToolStatus {
        MediaToolStatus {
            ffmpeg: self.ffmpeg.is_ok(),
            ffprobe: self.ffprobe.is_ok(),
            ffmpeg_path: path_string(&self.ffmpeg),
            ffprobe_path: path_string(&self.ffprobe),
            ffmpeg_error: failure_string(&self.ffmpeg),
            ffprobe_error: failure_string(&self.ffprobe),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_probe_output, resolve, MediaToolKind, MediaToolPaths, ResolutionContext,
        ResolveFailure,
    };
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    /// Context with no tools anywhere: nothing in the environment, no bundled
    /// directory, and a verifier that rejects everything. This is the state of
    /// a machine without ffmpeg installed.
    fn barren<'a>() -> ResolutionContext<'a> {
        ResolutionContext {
            env: &|_| None,
            bundled_dir: None,
            verify: &|_| false,
        }
    }

    fn env_from(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn reports_unavailable_rather_than_failing_when_no_binary_exists() {
        let ctx = barren();
        let error = resolve(MediaToolKind::Ffmpeg, &ctx).expect_err("nothing to find");
        assert!(matches!(error, ResolveFailure::NotFound { .. }));

        // The settings page renders a state, not an exception: flags are false,
        // paths absent, and each carries a reason.
        let status = MediaToolPaths::resolve(&ctx).to_status();
        assert!(!status.ffmpeg && !status.ffprobe);
        assert!(status.ffmpeg_path.is_none() && status.ffprobe_path.is_none());
        assert!(status
            .ffmpeg_error
            .is_some_and(|text| text.contains("ffmpeg")));
        assert!(status
            .ffprobe_error
            .is_some_and(|text| text.contains("ffprobe")));
    }

    #[test]
    fn rejects_override_pointing_at_a_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let as_string = dir.path().display().to_string();
        let env = env_from(&[("AUTOOWL_FFMPEG", &as_string)]);
        let ctx = ResolutionContext {
            env: &|name| env.get(name).cloned(),
            bundled_dir: None,
            // A directory must be refused even if the verifier is permissive.
            verify: &|_| true,
        };

        let error = resolve(MediaToolKind::Ffmpeg, &ctx).expect_err("directory is not a binary");
        match error {
            ResolveFailure::Directory { variable, path } => {
                assert_eq!(variable, "AUTOOWL_FFMPEG");
                assert_eq!(path, as_string);
            }
            other => panic!("expected Directory, got {other:?}"),
        }
    }

    #[test]
    fn rejects_override_that_will_not_execute() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fake = dir.path().join("ffmpeg");
        std::fs::write(&fake, b"not a binary").expect("write stub");
        let as_string = fake.display().to_string();
        let env = env_from(&[("AUTOOWL_FFMPEG", &as_string)]);
        let ctx = ResolutionContext {
            env: &|name| env.get(name).cloned(),
            bundled_dir: None,
            verify: &|_| false,
        };

        // Existence alone is not enough: an uninstall leftover would otherwise
        // be accepted and fail much later inside a render.
        let error = resolve(MediaToolKind::Ffmpeg, &ctx).expect_err("stub must be rejected");
        assert!(matches!(error, ResolveFailure::NotExecutable { .. }));
    }

    #[test]
    fn missing_override_target_is_reported_with_its_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let absent = dir.path().join("nope").join("ffprobe");
        let as_string = absent.display().to_string();
        let env = env_from(&[("AUTOOWL_FFPROBE", &as_string)]);
        let ctx = ResolutionContext {
            env: &|name| env.get(name).cloned(),
            bundled_dir: None,
            verify: &|_| true,
        };

        match resolve(MediaToolKind::Ffprobe, &ctx).expect_err("absent path") {
            ResolveFailure::Missing { variable, path } => {
                assert_eq!(variable, "AUTOOWL_FFPROBE");
                assert_eq!(path, as_string);
            }
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn bundled_binary_beats_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let bundled = dir.path().to_path_buf();
        let path_dir = dir.path().join("elsewhere");
        let env = env_from(&[("PATH", &path_dir.display().to_string())]);
        let ctx = ResolutionContext {
            env: &|name| env.get(name).cloned(),
            bundled_dir: Some(bundled.clone()),
            verify: &|candidate: &Path| {
                candidate
                    .file_stem()
                    .is_some_and(|stem| stem.eq_ignore_ascii_case("ffmpeg"))
            },
        };

        let resolved = resolve(MediaToolKind::Ffmpeg, &ctx).expect("resolved");
        assert_eq!(resolved.parent(), Some(bundled.as_path()));
    }

    #[test]
    fn falls_back_to_path_when_no_override_or_bundle() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path_dir = dir.path().to_path_buf();
        let env = env_from(&[("PATH", &path_dir.display().to_string())]);
        let ctx = ResolutionContext {
            env: &|name| env.get(name).cloned(),
            bundled_dir: None,
            verify: &|candidate: &Path| {
                candidate
                    .file_stem()
                    .is_some_and(|stem| stem.eq_ignore_ascii_case("ffprobe"))
            },
        };

        let resolved: PathBuf = resolve(MediaToolKind::Ffprobe, &ctx).expect("resolved");
        assert_eq!(resolved.parent(), Some(path_dir.as_path()));
    }

    #[test]
    fn finds_scoop_shims_missing_from_this_process_path() {
        // `scoop install` updates the user PATH, not the running app's, so the
        // shims directory is checked directly; `SCOOP` wins over the default.
        let dir = tempfile::tempdir().expect("temp dir");
        let custom = dir.path().join("custom");
        let shims = custom.join("shims");
        let env = env_from(&[
            ("PATH", &dir.path().join("empty").display().to_string()),
            ("SCOOP", &custom.display().to_string()),
            ("USERPROFILE", &dir.path().display().to_string()),
        ]);
        let ctx = ResolutionContext {
            env: &|name| env.get(name).cloned(),
            bundled_dir: None,
            verify: &|candidate: &Path| candidate.parent() == Some(shims.as_path()),
        };

        let resolved = resolve(MediaToolKind::Ffmpeg, &ctx).expect("resolved");
        assert_eq!(resolved.parent(), Some(shims.as_path()));
    }

    #[test]
    fn parses_ffprobe_json_into_metadata() {
        let json = r#"{
            "streams": [
                {"codec_type": "video", "duration": "1.0"},
                {"codec_type": "audio", "bit_rate": "320000", "tags": {"ALBUM": "流派"}}
            ],
            "format": {
                "duration": "215.960000",
                "tags": {"TITLE": "曲名", "artist": " 歌手 "}
            }
        }"#;

        let metadata = parse_probe_output(json).expect("parse");
        assert_eq!(metadata.title.as_deref(), Some("曲名"));
        assert_eq!(metadata.artist.as_deref(), Some("歌手"));
        // Container tags carry no album, so the audio stream supplies it.
        assert_eq!(metadata.album.as_deref(), Some("流派"));
        assert_eq!(metadata.duration_ms, Some(215_960));
        // Bitrate likewise falls back to the first audio stream.
        assert_eq!(metadata.bitrate, Some(320_000));
    }

    #[test]
    fn ignores_unusable_probe_values() {
        let json = r#"{"format": {"duration": "N/A", "bit_rate": "0", "tags": {"title": "  "}}}"#;
        let metadata = parse_probe_output(json).expect("parse");
        assert_eq!(metadata, super::MediaMetadata::default());
    }

    #[test]
    fn malformed_probe_output_is_an_error() {
        assert!(parse_probe_output("not json").is_err());
    }
}
