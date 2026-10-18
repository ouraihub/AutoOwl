use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum QrStatus {
    Pending,
    Scanned,
    Confirmed,
    Expired,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrSession {
    pub id: String,
    pub source: String,
    pub status: QrStatus,
}

pub fn transition(status: QrStatus, next: QrStatus) -> Result<QrStatus, String> {
    let valid = matches!(
        (status, next),
        (QrStatus::Pending, QrStatus::Scanned)
            | (QrStatus::Pending, QrStatus::Expired)
            | (QrStatus::Scanned, QrStatus::Confirmed)
            | (QrStatus::Scanned, QrStatus::Expired)
            | (QrStatus::Pending, QrStatus::Failed)
            | (QrStatus::Scanned, QrStatus::Failed)
    );
    valid
        .then_some(next)
        .ok_or_else(|| "invalid qr status transition".to_owned())
}
