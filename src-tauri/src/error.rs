use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("source unavailable: {0}")]
    SourceUnavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub operation_id: String,
}

impl IpcError {
    pub fn new(code: &str, message: &str, retryable: bool, operation_id: String) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable,
            operation_id,
        }
    }
}
