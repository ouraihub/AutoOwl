use serde::{Deserialize, Serialize};

use super::source::SongRef;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayableResource {
    pub song: SongRef,
    pub url: String,
    pub quality: String,
    pub duration_ms: Option<u64>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub original: Option<String>,
    pub translated: Option<String>,
    pub romanized: Option<String>,
    pub lines: Vec<LyricLine>,
}
