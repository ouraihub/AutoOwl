//! Portable ffmpeg for machines without Scoop (docs/概要设计.md §5.6).
//!
//! One pinned GyanD essentials build: version, size and SHA256 are constants,
//! so the optional GitHub proxy (a third party) cannot change what gets run.
//! The archive is verified before anything is extracted, and only
//! `ffmpeg.exe` / `ffprobe.exe` leave it, into `<data>\tools\ffmpeg`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

pub const FFMPEG_VERSION: &str = "9.0.2";
const ASSET_URL: &str =
    "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip";
/// Bytes of the asset, from the GitHub release metadata.
pub const ASSET_SIZE: u64 = 114_768_076;
const ASSET_SHA256: &str = "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba";
/// Archive members to keep; everything else (docs, ffplay) stays in the zip.
const MEMBERS: [&str; 2] = [
    "ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe",
    "ffmpeg-9.0.2-essentials_build/bin/ffprobe.exe",
];

/// Download location: direct, or behind the user's GitHub proxy prefix
/// (same `<prefix>/<url>` form the update check uses).
pub fn asset_url(proxy: Option<&str>) -> String {
    match proxy.map(str::trim).filter(|value| !value.is_empty()) {
        Some(prefix) => format!("{}/{ASSET_URL}", prefix.trim_end_matches('/')),
        None => ASSET_URL.to_owned(),
    }
}

/// Downloads, verifies and extracts into `target` (`<data>\tools\ffmpeg`).
/// `progress(received, total)` is called as bytes arrive.
pub async fn install(
    target: &Path,
    proxy: Option<&str>,
    progress: impl Fn(u64, u64),
) -> Result<(), String> {
    tokio::fs::create_dir_all(target)
        .await
        .map_err(|error| format!("无法创建 {}：{error}", target.display()))?;
    let archive = target.join("ffmpeg-download.zip.part");
    let url = asset_url(proxy);
    tracing::info!(event = "ffmpeg_download_start", %url);

    let result = async {
        let digest = download(&url, &archive, &progress).await?;
        verify(&digest)?;
        extract(&archive, target).await
    }
    .await;
    // The archive is never kept: on success it is spent, on failure untrusted.
    let _ = tokio::fs::remove_file(&archive).await;
    match &result {
        Ok(()) => tracing::info!(event = "ffmpeg_download_installed", path = %target.display()),
        Err(error) => tracing::warn!(event = "ffmpeg_download_failed", %error),
    }
    result
}

/// Streams the asset to `archive`, hashing as it goes; returns the hex SHA256.
async fn download(
    url: &str,
    archive: &Path,
    progress: &impl Fn(u64, u64),
) -> Result<String, String> {
    // The shared client's 30 s total timeout would cut off a 109 MB download;
    // a stalled read is what should fail, not a slow link.
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("下载 ffmpeg 失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "下载 ffmpeg 失败：HTTP {}",
            response.status().as_u16()
        ));
    }
    let mut file = tokio::fs::File::create(archive)
        .await
        .map_err(|error| format!("无法写入 {}：{error}", archive.display()))?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("下载 ffmpeg 中断：{error}"))?;
        // Bound the write: a proxy that streams forever must not fill the disk.
        received += chunk.len() as u64;
        if received > ASSET_SIZE {
            return Err("下载内容比预期大，已中止（代理返回的不是 ffmpeg 安装包）".to_owned());
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("写入下载文件失败：{error}"))?;
        progress(received, ASSET_SIZE);
    }
    file.flush().await.map_err(|error| error.to_string())?;
    Ok(hex(&hasher.finalize()))
}

/// The pinned digest is the only thing that makes proxied bytes trustworthy.
fn verify(digest: &str) -> Result<(), String> {
    if digest.eq_ignore_ascii_case(ASSET_SHA256) {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg 安装包校验失败（SHA256 不一致），已删除。若开启了 GitHub 加速代理，请换一个代理或关闭后重试。\n期望 {ASSET_SHA256}\n实际 {digest}"
        ))
    }
}

/// Extracts the two binaries with Windows' own `tar.exe` (bsdtar, reads zip,
/// present since Windows 10 1803), then flattens them into `target`.
async fn extract(archive: &Path, target: &Path) -> Result<(), String> {
    let tar = system_tar();
    let mut command = tokio::process::Command::new(&tar);
    command
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(target)
        .args(MEMBERS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let output = command
        .output()
        .await
        .map_err(|error| format!("无法运行 {}：{error}", tar.display()))?;
    if !output.status.success() {
        let log = String::from_utf8_lossy(&output.stderr);
        return Err(format!("解压 ffmpeg 失败：{}", log.trim()));
    }
    for member in MEMBERS {
        let extracted = target.join(member);
        let name = extracted.file_name().map(PathBuf::from).unwrap_or_default();
        tokio::fs::rename(&extracted, target.join(&name))
            .await
            .map_err(|error| format!("安装 {} 失败：{error}", name.display()))?;
    }
    if let Some(top) = MEMBERS[0].split('/').next() {
        let _ = tokio::fs::remove_dir_all(target.join(top)).await;
    }
    Ok(())
}

/// `%SystemRoot%\System32\tar.exe`, not whatever `tar` is first on PATH
/// (Git for Windows ships a GNU tar that cannot read zip).
fn system_tar() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
        .join("tar.exe")
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_prefixes_the_pinned_url_and_blank_means_direct() {
        assert_eq!(asset_url(None), ASSET_URL);
        assert_eq!(asset_url(Some("  ")), ASSET_URL);
        assert_eq!(
            asset_url(Some("https://edgeone.gh-proxy.com/")),
            format!("https://edgeone.gh-proxy.com/{ASSET_URL}")
        );
    }

    #[test]
    fn only_the_pinned_digest_is_accepted() {
        assert!(verify(ASSET_SHA256).is_ok());
        assert!(verify(&ASSET_SHA256.to_uppercase()).is_ok());
        let tampered = hex(&Sha256::digest(b"not ffmpeg"));
        assert!(verify(&tampered).unwrap_err().contains("SHA256"));
    }

    /// Real 109 MB download: proves URL, size, digest and archive member paths.
    #[tokio::test]
    #[ignore = "downloads the real ffmpeg release"]
    async fn installs_live_portable_ffmpeg() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), None, |_, _| {}).await.unwrap();
        for name in ["ffmpeg.exe", "ffprobe.exe"] {
            let out = std::process::Command::new(dir.path().join(name))
                .arg("-version")
                .output()
                .unwrap();
            assert!(String::from_utf8_lossy(&out.stdout).contains(FFMPEG_VERSION));
        }
        assert!(!dir.path().join("ffmpeg-download.zip.part").exists());
    }
}
