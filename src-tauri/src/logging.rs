use std::path::{Path, PathBuf};

use aether_observability::{LogConfig, LogHandle, LogLevel};

/// Bundle identifier from `tauri.conf.json`; Tauri's `app_data_dir` is `%APPDATA%\<id>`.
const IDENTIFIER: &str = "com.autoowl.desktop";

/// Same directory Tauri's `app_data_dir()` returns, but available before the
/// builder exists, so logging can start first and catch plugin setup.
pub fn data_root() -> PathBuf {
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(|dir| PathBuf::from(dir).join(IDENTIFIER))
        .unwrap_or_else(|| PathBuf::from("data"))
}

/// Rotating `logs/autoowl.log` (5 MB x 3); `RUST_LOG` overrides the level, and
/// the settings page can change it at runtime through the Aether plugin.
pub fn init(root: &Path) -> Result<LogHandle, String> {
    aether_observability::init(
        &root.join("logs"),
        LogConfig {
            file_stem: "autoowl",
            default_level: LogLevel::INFO,
            env_var: Some("RUST_LOG"),
            ..LogConfig::default()
        },
        None,
    )
    .map_err(|error| error.to_string())
}
