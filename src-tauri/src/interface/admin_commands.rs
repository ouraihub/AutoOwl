use crate::error::IpcError;
use crate::infrastructure::credentials::AuthStatus;
use crate::state::AppState;

/// Whether a source can be logged into with a QR code. Asks the adapter itself
/// rather than a hardcoded list, so the accounts page can never offer a scan
/// button for a source with no implementation behind it.
fn qr_supported(state: &AppState, source: &str) -> bool {
    state
        .sources
        .get(source)
        .map(|adapter| adapter.capabilities().qr_login)
        .unwrap_or(false)
}

#[tauri::command]
pub fn get_auth_status(
    state: tauri::State<'_, AppState>,
    source: String,
) -> Result<AuthStatus, IpcError> {
    if !state.sources.supports(&source) {
        return Err(unsupported_source());
    }
    Ok(state.cookies.status(&source, qr_supported(&state, &source)))
}

#[tauri::command]
pub fn save_source_cookie(
    state: tauri::State<'_, AppState>,
    source: String,
    cookie: String,
) -> Result<AuthStatus, IpcError> {
    if !state.sources.supports(&source) {
        return Err(unsupported_source());
    }
    state.cookies.save(&source, &cookie).map_err(|_| {
        IpcError::new(
            "INVALID_ARGUMENT",
            "Cookie 无效",
            false,
            "validation".to_owned(),
        )
    })?;
    Ok(state.cookies.status(&source, qr_supported(&state, &source)))
}

#[tauri::command]
pub fn clear_source_cookie(
    state: tauri::State<'_, AppState>,
    source: String,
) -> Result<AuthStatus, IpcError> {
    if !state.sources.supports(&source) {
        return Err(unsupported_source());
    }
    state.cookies.clear(&source).map_err(|_| registry_error())?;
    Ok(state.cookies.status(&source, qr_supported(&state, &source)))
}

/// Lists every source with its login state, for the accounts page.
#[tauri::command]
pub fn list_auth_status(state: tauri::State<'_, AppState>) -> Result<Vec<AuthStatus>, IpcError> {
    Ok(state
        .sources
        .ids()
        .into_iter()
        .map(|id| state.cookies.status(&id, qr_supported(&state, &id)))
        .collect())
}

fn unsupported_source() -> IpcError {
    IpcError::new(
        "SOURCE_NOT_FOUND",
        "音乐源不存在",
        false,
        "validation".to_owned(),
    )
}

fn registry_error() -> IpcError {
    IpcError::new(
        "STATE_UNAVAILABLE",
        "凭据状态暂时不可用",
        true,
        "credentials".to_owned(),
    )
}
