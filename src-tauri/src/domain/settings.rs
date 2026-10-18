//! Persisted user settings.
//!
//! `normalize` is the single place defaults and clamps are applied, so a value
//! loaded from an older database, hand-edited, or sent by a buggy caller lands
//! in range instead of reaching the download engine.

use serde::{Deserialize, Serialize};

/// Falls back to `<data>/downloads` via `AppPaths::resolve_download_dir`.
pub const DEFAULT_DOWNLOAD_DIR: &str = "downloads";
pub const DEFAULT_FILENAME_TEMPLATE: &str = "{artist} - {name}";
/// Where "check for updates" looks; this app's own repository.
pub const DEFAULT_UPDATE_REPO: &str = "https://github.com/ouraihub/AutoOwl";
pub const DEFAULT_GITHUB_PROXY: &str = "https://edgeone.gh-proxy.com";
pub const DEFAULT_CONCURRENCY: u8 = 3;
pub const DEFAULT_PAGE_SIZE: u32 = 20;

const MIN_CONCURRENCY: u8 = 1;
const MAX_CONCURRENCY: u8 = 5;
const MIN_PAGE_SIZE: u32 = 10;
const MAX_PAGE_SIZE: u32 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub download_dir: String,
    pub download_filename_template: String,
    pub embed_download: bool,
    pub download_concurrency: u8,
    pub auto_switch_invalid_sources: bool,
    pub auto_cache_on_play: bool,
    /// Suppresses the floating lyrics overlay.
    pub disable_floating_lyrics: bool,
    /// Rows per page in search and listing views.
    pub page_size: u32,
    pub auto_check_update: bool,
    pub update_repo_url: String,
    pub github_proxy_enabled: bool,
    pub github_proxy_url: String,
    /// Video export: allow swapping the cover.
    pub vg_change_cover: bool,
    /// Video export: allow swapping the audio track.
    pub vg_change_audio: bool,
    /// Video export: allow swapping the lyrics.
    pub vg_change_lyric: bool,
    /// Video export: write the mp4 rather than only previewing.
    pub vg_export_video: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            download_dir: DEFAULT_DOWNLOAD_DIR.to_owned(),
            download_filename_template: DEFAULT_FILENAME_TEMPLATE.to_owned(),
            embed_download: true,
            download_concurrency: DEFAULT_CONCURRENCY,
            auto_switch_invalid_sources: true,
            auto_cache_on_play: false,
            disable_floating_lyrics: false,
            page_size: DEFAULT_PAGE_SIZE,
            auto_check_update: true,
            update_repo_url: DEFAULT_UPDATE_REPO.to_owned(),
            github_proxy_enabled: false,
            github_proxy_url: DEFAULT_GITHUB_PROXY.to_owned(),
            vg_change_cover: false,
            vg_change_audio: false,
            vg_change_lyric: false,
            vg_export_video: false,
        }
    }
}

impl AppSettings {
    /// Substitutes defaults for blank strings and clamps numeric ranges.
    ///
    /// Concurrency is clamped rather than rejected because the value reaching
    /// the download semaphore as 0 would deadlock every transfer.
    pub fn normalize(mut self) -> Self {
        self.download_dir = default_if_blank(self.download_dir, DEFAULT_DOWNLOAD_DIR);
        self.download_filename_template =
            default_if_blank(self.download_filename_template, DEFAULT_FILENAME_TEMPLATE);
        self.update_repo_url = default_if_blank(self.update_repo_url, DEFAULT_UPDATE_REPO);
        self.github_proxy_url = default_if_blank(self.github_proxy_url, DEFAULT_GITHUB_PROXY);
        self.download_concurrency = self
            .download_concurrency
            .clamp(MIN_CONCURRENCY, MAX_CONCURRENCY);
        self.page_size = self.page_size.clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE);
        self
    }

    /// Proxy prefix for update checks, or `None` when disabled.
    pub fn update_proxy(&self) -> Option<&str> {
        self.github_proxy_enabled
            .then(|| self.github_proxy_url.trim())
            .filter(|value| !value.is_empty())
    }
}

/// Trims, then substitutes `fallback` when nothing is left.
fn default_if_blank(value: String, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppSettings, DEFAULT_DOWNLOAD_DIR, DEFAULT_FILENAME_TEMPLATE, DEFAULT_GITHUB_PROXY,
        DEFAULT_UPDATE_REPO,
    };

    #[test]
    fn blank_strings_fall_back_to_the_documented_defaults() {
        let settings = AppSettings {
            download_dir: "  ".into(),
            download_filename_template: String::new(),
            update_repo_url: "\t".into(),
            github_proxy_url: String::new(),
            ..Default::default()
        }
        .normalize();
        assert_eq!(settings.download_dir, DEFAULT_DOWNLOAD_DIR);
        assert_eq!(
            settings.download_filename_template,
            DEFAULT_FILENAME_TEMPLATE
        );
        assert_eq!(settings.update_repo_url, DEFAULT_UPDATE_REPO);
        assert_eq!(settings.github_proxy_url, DEFAULT_GITHUB_PROXY);
    }

    #[test]
    fn surrounding_whitespace_is_stripped_from_kept_values() {
        let settings = AppSettings {
            download_dir: "  D:/Music  ".into(),
            ..Default::default()
        }
        .normalize();
        assert_eq!(settings.download_dir, "D:/Music");
    }

    #[test]
    fn zero_concurrency_is_clamped_up_because_it_would_deadlock_downloads() {
        let settings = AppSettings {
            download_concurrency: 0,
            ..Default::default()
        }
        .normalize();
        assert_eq!(settings.download_concurrency, 1);
    }

    #[test]
    fn out_of_range_numbers_are_clamped_to_the_supported_window() {
        let settings = AppSettings {
            download_concurrency: 99,
            page_size: 5000,
            ..Default::default()
        }
        .normalize();
        assert_eq!(settings.download_concurrency, 5);
        assert_eq!(settings.page_size, 100);

        let small = AppSettings {
            page_size: 1,
            ..Default::default()
        }
        .normalize();
        assert_eq!(small.page_size, 10);
    }

    #[test]
    fn the_update_proxy_is_only_offered_when_enabled_and_non_blank() {
        let disabled = AppSettings::default();
        assert_eq!(disabled.update_proxy(), None);

        let enabled = AppSettings {
            github_proxy_enabled: true,
            ..Default::default()
        };
        assert_eq!(enabled.update_proxy(), Some(DEFAULT_GITHUB_PROXY));

        // Enabled but blank must not produce a "//" prefixed URL.
        let blank = AppSettings {
            github_proxy_enabled: true,
            github_proxy_url: "   ".into(),
            ..Default::default()
        };
        assert_eq!(blank.update_proxy(), None);
    }
}
