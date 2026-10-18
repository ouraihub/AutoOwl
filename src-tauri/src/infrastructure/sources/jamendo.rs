use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::domain::media::PlayableResource;
use crate::domain::source::{
    AlbumRef, Artist, MusicSource, SearchQuery, SearchResult, Song, SongRef, SourceCapabilities,
    SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::HttpClient;

const SEARCH_API: &str = "https://www.jamendo.com/api/search";
const SEARCH_PATH: &str = "/api/search";
const TRACK_API: &str = "https://www.jamendo.com/api/tracks";
const TRACK_PATH: &str = "/api/tracks";
const X_JAM_VERSION: &str = "4gvfvv";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/143.0.0.0 Safari/537.36";

pub struct JamendoSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> JamendoSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for JamendoSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "jamendo"
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            song_search: true,
            playlist_search: false,
            album_search: false,
            lyrics: false,
            cover: false,
            cookie: false,
            qr_login: false,
            qualities: vec![
                "flac".into(),
                "mp33".into(),
                "mp32".into(),
                "mp3".into(),
                "ogg".into(),
            ],
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let mut url = reqwest::Url::parse(SEARCH_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("query", &query.query)
            .append_pair("type", "track")
            .append_pair("limit", &query.page_size.to_string())
            .append_pair("identities", "www");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| format!("0.{}", duration.subsec_nanos()))
            .unwrap_or_else(|_| "0.5".into());
        let response = self
            .client
            .get_with_headers(
                url.as_str(),
                &[
                    ("User-Agent", USER_AGENT.into()),
                    ("Referer", "https://www.jamendo.com/search?q=musicdl".into()),
                    ("x-jam-call", make_x_jam_call(SEARCH_PATH, &nonce)),
                    ("x-jam-version", X_JAM_VERSION.into()),
                    ("x-requested-with", "XMLHttpRequest".into()),
                    ("Cookie", self.cookie.value()),
                ],
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("jamendo returned HTTP {}", response.status),
                response.status == 429 || response.status >= 500,
            ));
        }
        parse_search(&response.body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        if song.source != self.id() || song.scope != "song" || song.site_id.trim().is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid Jamendo song reference".into(),
                false,
            ));
        }
        let mut url = reqwest::Url::parse(TRACK_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut().append_pair("id", &song.site_id);
        let response = self.api_get(url.as_str(), TRACK_PATH).await?;
        parse_playable(&response.body, song)
    }
}

impl<C> JamendoSource<C>
where
    C: HttpClient,
{
    async fn api_get(
        &self,
        url: &str,
        path: &str,
    ) -> Result<crate::infrastructure::http::HttpResponse, SourceError> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| format!("0.{}", duration.subsec_nanos()))
            .unwrap_or_else(|_| "0.5".into());
        let response = self
            .client
            .get_with_headers(
                url,
                &[
                    ("User-Agent", USER_AGENT.into()),
                    ("Referer", "https://www.jamendo.com/search?q=musicdl".into()),
                    ("x-jam-call", make_x_jam_call(path, &nonce)),
                    ("x-jam-version", X_JAM_VERSION.into()),
                    ("x-requested-with", "XMLHttpRequest".into()),
                    ("Cookie", self.cookie.value()),
                ],
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("jamendo returned HTTP {}", response.status),
                response.status == 429 || response.status >= 500,
            ));
        }
        Ok(response)
    }
}

fn make_x_jam_call(path: &str, nonce: &str) -> String {
    let digest = Sha1::digest(format!("{path}{nonce}").as_bytes());
    format!("${}*{nonce}~", hex::encode(digest))
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    let tracks: Vec<SearchTrack> = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let items = tracks
        .into_iter()
        .filter(|track| !track.download.is_empty() || !track.stream.is_empty())
        .map(|track| Song {
            reference: SongRef {
                source: "jamendo".into(),
                scope: "song".into(),
                site_id: track.id.to_string(),
            },
            title: track.name,
            artists: vec![Artist {
                id: (track.artist.id > 0).then(|| track.artist.id.to_string()),
                name: track.artist.name,
            }],
            album: Some(AlbumRef {
                id: (track.album.id > 0).then(|| track.album.id.to_string()),
                name: track.album.name,
                source: Some("jamendo".into()),
            }),
            duration_ms: Some((track.duration as u64).saturating_mul(1000)),
            cover_url: (!track.cover.big.size_300.is_empty()).then_some(track.cover.big.size_300),
        })
        .collect();
    Ok(SearchResult {
        items,
        total: None,
        page,
        page_size,
    })
}

fn parse_playable(body: &[u8], song: SongRef) -> Result<PlayableResource, SourceError> {
    let tracks: Vec<SearchTrack> = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let track = tracks.into_iter().next().ok_or_else(|| {
        source_error(
            "SOURCE_TRACK_NOT_FOUND",
            "Jamendo track not found".into(),
            false,
        )
    })?;
    let streams = if track.download.is_empty() {
        &track.stream
    } else {
        &track.download
    };
    let (quality, url) = ["flac", "mp33", "mp32", "mp3", "ogg"]
        .into_iter()
        .find_map(|quality| {
            streams
                .get(quality)
                .filter(|url| !url.trim().is_empty())
                .map(|url| (quality, url.clone()))
        })
        .ok_or_else(|| {
            source_error(
                "SOURCE_PLAYBACK_UNAVAILABLE",
                "Jamendo track has no playable stream".into(),
                false,
            )
        })?;
    Ok(PlayableResource {
        song,
        url,
        quality: quality.into(),
        duration_ms: Some((track.duration as u64).saturating_mul(1000)),
        expires_at: None,
    })
}

fn source_error(code: &str, message: String, retryable: bool) -> SourceError {
    SourceError {
        code: code.into(),
        message,
        retryable,
    }
}

#[derive(Deserialize)]
struct SearchTrack {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    duration: u32,
    #[serde(default)]
    artist: SearchNamedItem,
    #[serde(default)]
    album: SearchNamedItem,
    #[serde(default)]
    cover: SearchCover,
    #[serde(default)]
    download: HashMap<String, String>,
    #[serde(default)]
    stream: HashMap<String, String>,
}

#[derive(Default, Deserialize)]
struct SearchNamedItem {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct SearchCover {
    #[serde(default)]
    big: SearchCoverBig,
}

#[derive(Default, Deserialize)]
struct SearchCoverBig {
    #[serde(rename = "size300", default)]
    size_300: String,
}

#[cfg(test)]
mod tests {
    use super::{make_x_jam_call, parse_playable, parse_search, JamendoSource};
    use crate::domain::source::{MusicSource, SearchQuery, SongRef};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    #[test]
    fn signs_fixed_nonce() {
        assert_eq!(
            make_x_jam_call("/api/search", "0.5"),
            "$b3b3fd94650d2b10acbb75b3b8401ca82c06046c*0.5~"
        );
    }

    #[test]
    fn parses_fixture_and_filters_track_without_stream() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/jamendo/search.json"),
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].reference.site_id, "1001");
        assert_eq!(
            result.items[0]
                .album
                .as_ref()
                .and_then(|album| album.id.as_deref()),
            Some("3001")
        );
    }

    #[test]
    fn resolves_best_fixture_stream() {
        let song = SongRef {
            source: "jamendo".into(),
            scope: "song".into(),
            site_id: "1001".into(),
        };
        let resource = parse_playable(
            include_bytes!("../../../../tests/fixtures/jamendo/search.json"),
            song,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(resource.quality, "mp3");
        assert_eq!(resource.url, "https://example.test/song.mp3");
    }

    #[tokio::test]
    #[ignore = "requires live Jamendo service"]
    async fn resolves_live_resource() {
        let source = JamendoSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let search = source
            .search_songs(SearchQuery {
                query: "rock".into(),
                page: 1,
                page_size: 5,
                sources: vec!["jamendo".into()],
            })
            .await
            .unwrap_or_else(|_| unreachable!());
        let song = search.items[0].reference.clone();
        let result = source.resolve_song(song).await;
        assert!(result.is_ok(), "{result:?}");
        assert!(result
            .unwrap_or_else(|_| unreachable!())
            .url
            .starts_with("http"));
    }

    #[tokio::test]
    #[ignore = "requires live Jamendo service"]
    async fn searches_live_service() {
        let source = JamendoSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "rock".into(),
                page: 1,
                page_size: 20,
                sources: vec!["jamendo".into()],
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }
}
