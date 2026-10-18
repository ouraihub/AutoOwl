//! Settings persistence.
//!
//! `app_settings` is a KV table keyed by setting name in `data/settings.db`.
//! Storing one row per field (rather than one JSON blob) keeps a schema
//! addition from invalidating stored values.

use crate::domain::clock::now_iso8601;
use crate::domain::settings::AppSettings;
use crate::infrastructure::storage::{Database, StorageError};

const KEY_DOWNLOAD_DIR: &str = "download_dir";
const KEY_FILENAME_TEMPLATE: &str = "download_filename_template";
const KEY_EMBED_DOWNLOAD: &str = "embed_download";
const KEY_CONCURRENCY: &str = "download_concurrency";
const KEY_AUTO_SWITCH: &str = "auto_switch_invalid_sources";
const KEY_AUTO_CACHE: &str = "auto_cache_on_play";
const KEY_DISABLE_FLOATING_LYRICS: &str = "disable_floating_lyrics";
const KEY_PAGE_SIZE: &str = "page_size";
const KEY_AUTO_CHECK_UPDATE: &str = "auto_check_update";
const KEY_UPDATE_REPO_URL: &str = "update_repo_url";
const KEY_GITHUB_PROXY_ENABLED: &str = "github_proxy_enabled";
const KEY_GITHUB_PROXY_URL: &str = "github_proxy_url";
const KEY_VG_CHANGE_COVER: &str = "vg_change_cover";
const KEY_VG_CHANGE_AUDIO: &str = "vg_change_audio";
const KEY_VG_CHANGE_LYRIC: &str = "vg_change_lyric";
const KEY_VG_EXPORT_VIDEO: &str = "vg_export_video";

/// Loads settings, falling back to defaults for missing or corrupt values.
pub fn load(database: &Database) -> Result<AppSettings, StorageError> {
    let stored = database.with(|connection| {
        let mut statement = connection.prepare("SELECT key, value_json FROM app_settings")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<std::collections::HashMap<_, _>, _>>()
    })?;

    let defaults = AppSettings::default();
    let text = |key: &str, fallback: String| {
        stored
            .get(key)
            .and_then(|value| serde_json::from_str::<String>(value).ok())
            .unwrap_or(fallback)
    };
    let flag = |key: &str, fallback: bool| {
        stored
            .get(key)
            .and_then(|value| serde_json::from_str::<bool>(value).ok())
            .unwrap_or(fallback)
    };
    let number = |key: &str, fallback: u32| {
        stored
            .get(key)
            .and_then(|value| serde_json::from_str::<u32>(value).ok())
            .unwrap_or(fallback)
    };
    Ok(AppSettings {
        download_dir: text(KEY_DOWNLOAD_DIR, defaults.download_dir),
        download_filename_template: text(
            KEY_FILENAME_TEMPLATE,
            defaults.download_filename_template,
        ),
        embed_download: flag(KEY_EMBED_DOWNLOAD, defaults.embed_download),
        download_concurrency: stored
            .get(KEY_CONCURRENCY)
            .and_then(|value| serde_json::from_str::<u8>(value).ok())
            .unwrap_or(defaults.download_concurrency),
        auto_switch_invalid_sources: flag(KEY_AUTO_SWITCH, defaults.auto_switch_invalid_sources),
        auto_cache_on_play: flag(KEY_AUTO_CACHE, defaults.auto_cache_on_play),
        disable_floating_lyrics: flag(
            KEY_DISABLE_FLOATING_LYRICS,
            defaults.disable_floating_lyrics,
        ),
        page_size: number(KEY_PAGE_SIZE, defaults.page_size),
        auto_check_update: flag(KEY_AUTO_CHECK_UPDATE, defaults.auto_check_update),
        update_repo_url: text(KEY_UPDATE_REPO_URL, defaults.update_repo_url),
        github_proxy_enabled: flag(KEY_GITHUB_PROXY_ENABLED, defaults.github_proxy_enabled),
        github_proxy_url: text(KEY_GITHUB_PROXY_URL, defaults.github_proxy_url),
        vg_change_cover: flag(KEY_VG_CHANGE_COVER, defaults.vg_change_cover),
        vg_change_audio: flag(KEY_VG_CHANGE_AUDIO, defaults.vg_change_audio),
        vg_change_lyric: flag(KEY_VG_CHANGE_LYRIC, defaults.vg_change_lyric),
        vg_export_video: flag(KEY_VG_EXPORT_VIDEO, defaults.vg_export_video),
    }
    .normalize())
}

/// Replaces every stored setting in one transaction.
pub fn save(database: &Database, settings: &AppSettings) -> Result<(), StorageError> {
    let timestamp = now_iso8601();
    let entries = [
        (
            KEY_DOWNLOAD_DIR,
            serde_json::Value::from(settings.download_dir.clone()),
        ),
        (
            KEY_FILENAME_TEMPLATE,
            serde_json::Value::from(settings.download_filename_template.clone()),
        ),
        (
            KEY_EMBED_DOWNLOAD,
            serde_json::Value::from(settings.embed_download),
        ),
        (
            KEY_CONCURRENCY,
            serde_json::Value::from(settings.download_concurrency),
        ),
        (
            KEY_AUTO_SWITCH,
            serde_json::Value::from(settings.auto_switch_invalid_sources),
        ),
        (
            KEY_AUTO_CACHE,
            serde_json::Value::from(settings.auto_cache_on_play),
        ),
        (
            KEY_DISABLE_FLOATING_LYRICS,
            serde_json::Value::from(settings.disable_floating_lyrics),
        ),
        (KEY_PAGE_SIZE, serde_json::Value::from(settings.page_size)),
        (
            KEY_AUTO_CHECK_UPDATE,
            serde_json::Value::from(settings.auto_check_update),
        ),
        (
            KEY_UPDATE_REPO_URL,
            serde_json::Value::from(settings.update_repo_url.clone()),
        ),
        (
            KEY_GITHUB_PROXY_ENABLED,
            serde_json::Value::from(settings.github_proxy_enabled),
        ),
        (
            KEY_GITHUB_PROXY_URL,
            serde_json::Value::from(settings.github_proxy_url.clone()),
        ),
        (
            KEY_VG_CHANGE_COVER,
            serde_json::Value::from(settings.vg_change_cover),
        ),
        (
            KEY_VG_CHANGE_AUDIO,
            serde_json::Value::from(settings.vg_change_audio),
        ),
        (
            KEY_VG_CHANGE_LYRIC,
            serde_json::Value::from(settings.vg_change_lyric),
        ),
        (
            KEY_VG_EXPORT_VIDEO,
            serde_json::Value::from(settings.vg_export_video),
        ),
    ];
    database.with_transaction(|transaction| {
        for (key, value) in entries {
            transaction.execute(
                "INSERT INTO app_settings(key, value_json, updated_at) VALUES(?1, ?2, ?3) \
                 ON CONFLICT(key) DO UPDATE SET \
                 value_json = excluded.value_json, updated_at = excluded.updated_at",
                rusqlite::params![key, value.to_string(), timestamp],
            )?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{load, save};
    use crate::domain::settings::AppSettings;
    use crate::infrastructure::storage::Database;

    #[test]
    fn defaults_apply_to_an_empty_database() {
        let database = Database::open_in_memory().expect("open");
        let settings = load(&database).expect("load");
        assert_eq!(settings.download_dir, AppSettings::default().download_dir);
    }

    /// Every field must survive the trip. A field added to `AppSettings` but
    /// forgotten in `save`'s entry list silently reverts to its default on the
    /// next load, which is exactly the bug this asserts against: every value
    /// below differs from the default.
    #[test]
    fn saved_settings_round_trip() {
        let database = Database::open_in_memory().expect("open");
        let settings = AppSettings {
            download_dir: "D:/Music".to_owned(),
            download_filename_template: "{album}/{name}".to_owned(),
            embed_download: false,
            download_concurrency: 4,
            auto_switch_invalid_sources: false,
            auto_cache_on_play: true,
            disable_floating_lyrics: true,
            page_size: 50,
            auto_check_update: false,
            update_repo_url: "https://example.invalid/owner/repo".to_owned(),
            github_proxy_enabled: true,
            github_proxy_url: "https://proxy.invalid".to_owned(),
            vg_change_cover: true,
            vg_change_audio: true,
            vg_change_lyric: true,
            vg_export_video: true,
        };
        save(&database, &settings).expect("save");
        let loaded = load(&database).expect("load");
        assert_eq!(loaded.download_dir, "D:/Music");
        assert_eq!(loaded.download_filename_template, "{album}/{name}");
        assert!(!loaded.embed_download);
        assert_eq!(loaded.download_concurrency, 4);
        assert!(!loaded.auto_switch_invalid_sources);
        assert!(loaded.auto_cache_on_play);
        assert!(loaded.disable_floating_lyrics);
        assert_eq!(loaded.page_size, 50);
        assert!(!loaded.auto_check_update);
        assert_eq!(loaded.update_repo_url, "https://example.invalid/owner/repo");
        assert!(loaded.github_proxy_enabled);
        assert_eq!(loaded.github_proxy_url, "https://proxy.invalid");
        assert!(loaded.vg_change_cover);
        assert!(loaded.vg_change_audio);
        assert!(loaded.vg_change_lyric);
        assert!(loaded.vg_export_video);
    }

    #[test]
    fn saving_twice_updates_in_place() {
        let database = Database::open_in_memory().expect("open");
        save(&database, &AppSettings::default()).expect("first");
        let settings = AppSettings {
            download_concurrency: 5,
            ..AppSettings::default()
        };
        save(&database, &settings).expect("second");
        let rows: i64 = database
            .with(|connection| {
                connection.query_row("SELECT COUNT(*) FROM app_settings", [], |row| row.get(0))
            })
            .expect("count");
        // One row per field, not one row per save: a missing ON CONFLICT clause
        // would double this.
        assert_eq!(rows, 16);
        assert_eq!(load(&database).expect("load").download_concurrency, 5);
    }

    #[test]
    fn corrupt_values_fall_back_to_defaults() {
        let database = Database::open_in_memory().expect("open");
        database
            .with(|connection| {
                connection.execute(
                    "INSERT INTO app_settings(key, value_json, updated_at) \
                     VALUES('download_concurrency', 'not-a-number', 'now')",
                    [],
                )
            })
            .expect("insert");
        assert_eq!(
            load(&database).expect("load").download_concurrency,
            AppSettings::default().download_concurrency
        );
    }

    #[test]
    fn loaded_values_are_normalized() {
        let database = Database::open_in_memory().expect("open");
        let settings = AppSettings {
            download_concurrency: 9,
            download_dir: "  ".to_owned(),
            ..AppSettings::default()
        };
        save(&database, &settings).expect("save");
        let loaded = load(&database).expect("load");
        assert_eq!(loaded.download_concurrency, 5);
        assert_eq!(loaded.download_dir, "downloads");
    }
}
