use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::media::{Lyrics, PlayableResource};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongRef {
    pub source: String,
    pub scope: String,
    pub site_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRef {
    pub id: Option<String>,
    pub name: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    #[serde(rename = "ref")]
    pub reference: SongRef,
    pub title: String,
    pub artists: Vec<Artist>,
    pub album: Option<AlbumRef>,
    pub duration_ms: Option<u64>,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub query: String,
    pub page: u32,
    pub page_size: u32,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub items: Vec<Song>,
    pub total: Option<u64>,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCapabilities {
    pub song_search: bool,
    pub playlist_search: bool,
    pub album_search: bool,
    pub lyrics: bool,
    pub cover: bool,
    pub cookie: bool,
    pub qr_login: bool,
    pub qualities: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SourceError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

/// Paged collection listing returned by playlist/album search.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionPage {
    pub items: Vec<super::collection::Collection>,
    pub total: Option<u64>,
    pub page: u32,
    pub page_size: u32,
}

/// A collection plus its tracks, as returned by detail/parse calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionDetail {
    pub collection: super::collection::Collection,
    pub songs: Vec<Song>,
}

/// Capabilities a source does not implement return
/// `SOURCE_CAPABILITY_UNSUPPORTED`, letting the aggregation layer skip the
/// source instead of failing the whole request.
#[async_trait]
pub trait MusicSource: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> SourceCapabilities;

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError>;

    /// Resolves a playable stream URL for a song.
    async fn resolve_song(&self, _song: SongRef) -> Result<PlayableResource, SourceError> {
        Err(unsupported(self.id(), "playback"))
    }

    /// Resolves the URL to download from; defaults to the playback URL because
    /// most sources serve one stream for both.
    async fn download_url(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        self.resolve_song(song).await
    }

    /// Fetches lyrics, including translations when the source provides them.
    async fn lyrics(&self, _song: SongRef) -> Result<Lyrics, SourceError> {
        Err(unsupported(self.id(), "lyrics"))
    }

    async fn search_playlists(&self, _query: SearchQuery) -> Result<CollectionPage, SourceError> {
        Err(unsupported(self.id(), "playlist search"))
    }

    async fn search_albums(&self, _query: SearchQuery) -> Result<CollectionPage, SourceError> {
        Err(unsupported(self.id(), "album search"))
    }

    /// Lists the tracks of a playlist.
    async fn playlist_detail(&self, _id: &str) -> Result<CollectionDetail, SourceError> {
        Err(unsupported(self.id(), "playlist detail"))
    }

    /// Lists the tracks of an album.
    async fn album_detail(&self, _id: &str) -> Result<CollectionDetail, SourceError> {
        Err(unsupported(self.id(), "album detail"))
    }
}

/// Error returned when a source lacks a capability.
pub fn unsupported(source: &str, capability: &str) -> SourceError {
    SourceError {
        code: "SOURCE_CAPABILITY_UNSUPPORTED".to_owned(),
        message: format!("{source} does not support {capability}"),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{AlbumRef, Artist, Song, SongRef};

    #[test]
    fn song_serializes_to_typescript_contract() {
        let song = Song {
            reference: SongRef {
                source: "netease".into(),
                scope: "song".into(),
                site_id: "42".into(),
            },
            title: "Example".into(),
            artists: vec![Artist {
                id: Some("7".into()),
                name: "Artist".into(),
            }],
            album: Some(AlbumRef {
                id: Some("9".into()),
                name: "Album".into(),
                source: Some("netease".into()),
            }),
            duration_ms: Some(180_000),
            cover_url: None,
        };

        let value = serde_json::to_value(song).unwrap_or_default();
        assert_eq!(value["ref"]["siteId"], "42");
        assert_eq!(value["artists"][0]["name"], "Artist");
        assert_eq!(value["album"]["name"], "Album");
        assert_eq!(value["durationMs"], 180_000);
        assert!(value.get("reference").is_none());
    }
}
