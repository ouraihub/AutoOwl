//! Environment report for the diagnostics page.
//!
//! Every field is observed, not assumed: the settings page uses this to tell a
//! user why something is broken, so a hardcoded `database_ready: true` would be
//! worse than no page at all.

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub app_version: String,
    pub platform: String,
    pub arch: String,
    pub data_dir: String,
    pub log_dir: String,
    pub download_dir: String,
    /// True when the database file exists and answers a trivial query.
    pub database_ready: bool,
}

/// Builds the report from the real paths and a live database probe.
pub fn current(paths: &crate::config::AppPaths, database_ready: bool) -> Diagnostics {
    Diagnostics {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        data_dir: display(&paths.root),
        log_dir: display(&paths.logs),
        download_dir: display(&paths.downloads),
        database_ready,
    }
}

/// Lossy on purpose: a non-UTF-8 path must still render in the UI.
fn display(path: &Path) -> String {
    path.display().to_string()
}
