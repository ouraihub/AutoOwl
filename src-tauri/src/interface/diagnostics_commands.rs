//! Diagnostics and update-check commands.
//!
//! Both back the settings/diagnostics page, whose only job is to explain a
//! broken environment, so both report observed state rather than defaults.

use tauri::State;

use crate::application::{diagnostics, update};
use crate::state::AppState;

#[tauri::command]
pub fn get_diagnostics(state: State<'_, AppState>) -> diagnostics::Diagnostics {
    // A trivial query is the only honest readiness signal: the file can exist
    // and still be locked or corrupt.
    let database_ready = state
        .database
        .with(|connection| connection.query_row("SELECT 1", [], |row| row.get::<_, i64>(0)))
        .is_ok();
    diagnostics::current(&state.paths, database_ready)
}

/// Checks GitHub for a newer release, using the repo and proxy from settings.
///
/// A network failure answers with the current version and `available: false`
/// rather than an error: "could not reach GitHub" is not "up to date", but it is
/// also not something the page should render as a failed operation.
///
/// Takes an owned `AppHandle` rather than `State<'_, _>` so the future borrows
/// nothing and the command can report a plain value instead of a never-used
/// `Result`.
#[tauri::command]
pub async fn check_update(app: tauri::AppHandle) -> update::UpdateInfo {
    // Snapshot and drop the state borrow before awaiting: `State` is not Send.
    let (http, repo, proxy) = {
        use tauri::Manager;
        let state = app.state::<AppState>();
        let settings = state.settings_snapshot();
        (
            state.http.clone(),
            settings.update_repo_url.clone(),
            settings.update_proxy().map(str::to_owned),
        )
    };
    match update::check(&http, &repo, proxy.as_deref()).await {
        Ok(info) => info,
        Err(error) => {
            tracing::warn!(event = "update_check_failed", %error);
            update::current()
        }
    }
}
