use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::application::tasks::TaskRegistry;
use crate::config::AppPaths;
use crate::domain::download::DownloadTask;
use crate::domain::local_music::LocalTrack;
use crate::domain::player::PlayerState;
use crate::domain::settings::AppSettings;
use crate::infrastructure::credentials::CookieStore;
use crate::infrastructure::http::ReqwestHttpClient;
use crate::infrastructure::sources::registry::SourceRegistry;
use crate::infrastructure::storage::Database;

pub struct AppState {
    pub paths: AppPaths,
    pub database: Arc<Database>,
    pub http: ReqwestHttpClient,
    pub cookies: Arc<CookieStore>,
    pub sources: SourceRegistry,
    pub task_registry: TaskRegistry,
    pub player: Mutex<PlayerState>,
    pub settings: Mutex<AppSettings>,
    pub local_tracks: Mutex<Vec<LocalTrack>>,
    pub downloads: Mutex<HashMap<String, DownloadTask>>,
    next_task_id: AtomicU64,
}

impl AppState {
    /// Builds application state on the resolved data/logs locations, persisting to SQLite.
    ///
    /// Falls back to an in-memory database when the database cannot be
    /// opened, so the UI still runs and reports the failure instead of the
    /// process dying at startup.
    pub fn new(locations: &crate::storage::Locations) -> Self {
        let paths = AppPaths::from_root(locations.data.clone()).with_logs(locations.logs.clone());
        if let Err(error) = paths.ensure() {
            tracing::error!(event = "data_dir_create_failed", %error);
        }
        let database = match Database::open(&paths.database) {
            Ok(database) => database,
            Err(error) => {
                tracing::error!(event = "database_open_failed", %error);
                Database::open_in_memory().unwrap_or_else(|fallback_error| {
                    tracing::error!(event = "database_memory_failed", error = %fallback_error);
                    panic!("sqlite is unusable: {fallback_error}")
                })
            }
        };
        let database = Arc::new(database);
        let cookies = Arc::new(CookieStore::with_database(Arc::clone(&database)));
        let settings = crate::infrastructure::settings_repository::load(&database)
            .unwrap_or_default()
            .normalize();
        Self {
            sources: SourceRegistry::with_sources(Arc::clone(&cookies)),
            paths,
            database,
            http: ReqwestHttpClient::default(),
            cookies,
            task_registry: TaskRegistry::default(),
            player: Mutex::default(),
            settings: Mutex::new(settings),
            local_tracks: Mutex::default(),
            downloads: Mutex::default(),
            next_task_id: AtomicU64::new(1),
        }
    }

    /// State backed by an in-memory database, for tests.
    pub fn in_memory() -> Self {
        let database = Arc::new(
            Database::open_in_memory().unwrap_or_else(|error| panic!("sqlite unusable: {error}")),
        );
        let cookies = Arc::new(CookieStore::with_database(Arc::clone(&database)));
        Self {
            sources: SourceRegistry::with_sources(Arc::clone(&cookies)),
            paths: AppPaths::from_root(std::env::temp_dir().join("autoowl-test")),
            database,
            http: ReqwestHttpClient::default(),
            cookies,
            task_registry: TaskRegistry::default(),
            player: Mutex::default(),
            settings: Mutex::default(),
            local_tracks: Mutex::default(),
            downloads: Mutex::default(),
            next_task_id: AtomicU64::new(1),
        }
    }

    /// Monotonic task id, unique for the process lifetime.
    pub fn next_task_id(&self, prefix: &str) -> String {
        let sequence = self.next_task_id.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{sequence}")
    }

    /// Download directory from the current settings.
    pub fn download_root(&self) -> std::path::PathBuf {
        let configured = self
            .settings
            .lock()
            .map(|settings| settings.download_dir.clone())
            .unwrap_or_default();
        self.paths.resolve_download_dir(&configured)
    }

    pub fn settings_snapshot(&self) -> AppSettings {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    /// Normalizes, saves to SQLite, then applies in memory. Saving first means a
    /// failed write leaves the running app on the settings that are on disk.
    pub fn update_settings(&self, settings: AppSettings) -> Result<AppSettings, String> {
        let normalized = settings.normalize();
        crate::infrastructure::settings_repository::save(&self.database, &normalized)
            .map_err(|error| error.to_string())?;
        *self
            .settings
            .lock()
            .map_err(|_| "settings lock poisoned".to_owned())? = normalized.clone();
        Ok(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::AppState;

    #[test]
    fn task_ids_are_unique_and_prefixed() {
        let state = AppState::in_memory();
        assert_eq!(state.next_task_id("dl"), "dl-1");
        assert_eq!(state.next_task_id("dl"), "dl-2");
        assert_eq!(state.next_task_id("render"), "render-3");
    }

    #[test]
    fn download_root_follows_settings() {
        let state = AppState::in_memory();
        if let Ok(mut settings) = state.settings.lock() {
            settings.download_dir = "music".to_owned();
        }
        assert!(state.download_root().ends_with("music"));
    }

    #[test]
    fn updated_settings_survive_a_restart() {
        let state = AppState::in_memory();
        let mut wanted = state.settings_snapshot();
        wanted.download_dir = r"E:\Music".to_owned();
        wanted.github_proxy_enabled = true;
        state.update_settings(wanted).unwrap();

        // What the next launch reads (`AppState::new` loads from the database).
        let reloaded = crate::infrastructure::settings_repository::load(&state.database).unwrap();
        assert_eq!(reloaded.download_dir, r"E:\Music");
        assert!(reloaded.github_proxy_enabled);
        assert_eq!(state.settings_snapshot().download_dir, r"E:\Music");
    }
}
