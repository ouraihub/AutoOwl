use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CollectionType {
    Playlist,
    Album,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub source: String,
    pub content_type: CollectionType,
    pub name: String,
    pub description: Option<String>,
    pub creator: Option<String>,
    pub cover_url: Option<String>,
    pub track_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionQuery {
    pub query: String,
    pub page: u32,
    pub page_size: u32,
}
