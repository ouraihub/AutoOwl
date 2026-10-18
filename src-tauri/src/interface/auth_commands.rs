use crate::application::qr_login::{self, QrLoginResult, QrLoginSession};
use crate::domain::auth::QrStatus;
use crate::error::IpcError;
use crate::state::AppState;
use tauri::State;

/// Starts a QR login session and returns the code as a data URL.
#[tauri::command]
pub async fn qr_login_create(
    state: State<'_, AppState>,
    source: String,
) -> Result<QrLoginSession, IpcError> {
    if !state.sources.supports(&source) {
        return Err(unsupported_source());
    }
    // Clone the client so nothing borrowed from `State` is held across the await.
    let http = state.http.clone();
    qr_login::create(&http, &source)
        .await
        .map_err(|error| qr_error("QR_LOGIN_FAILED", &error, true))
}

/// Polls a QR login session; persists the cookie once the user confirms.
#[tauri::command]
pub async fn qr_login_check(
    state: State<'_, AppState>,
    source: String,
    key: String,
) -> Result<QrLoginResult, IpcError> {
    if !state.sources.supports(&source) {
        return Err(unsupported_source());
    }
    if key.trim().is_empty() {
        return Err(IpcError::new(
            "INVALID_ARGUMENT",
            "扫码会话已失效，请重新获取二维码",
            false,
            "auth".to_owned(),
        ));
    }
    let http = state.http.clone();
    let result = qr_login::check(&http, &source, &key)
        .await
        .map_err(|error| qr_error("QR_LOGIN_FAILED", &error, true))?;

    // Persisting the cookie is the point of the feature: a confirmed scan that
    // is not stored leaves the source logged out.
    if matches!(result.status, QrStatus::Confirmed) {
        let cookie = result.cookie.as_deref().unwrap_or_default();
        if cookie.trim().is_empty() {
            return Err(qr_error(
                "QR_LOGIN_FAILED",
                "登录已确认但未返回凭据，请重试",
                true,
            ));
        }
        state
            .cookies
            .save(&source, cookie)
            .map_err(|error| qr_error("STATE_UNAVAILABLE", &error, true))?;
    }
    Ok(result)
}

fn qr_error(code: &str, message: &str, retryable: bool) -> IpcError {
    IpcError::new(code, message, retryable, "auth".to_owned())
}

fn unsupported_source() -> IpcError {
    IpcError::new("SOURCE_NOT_FOUND", "音乐源不存在", false, "auth".to_owned())
}
