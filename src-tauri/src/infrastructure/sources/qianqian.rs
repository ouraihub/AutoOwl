use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::media::PlayableResource;
use crate::domain::source::{
    AlbumRef, Artist, MusicSource, SearchQuery, SearchResult, Song, SongRef, SourceCapabilities,
    SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::HttpClient;

const SEARCH_API: &str = "https://music.91q.com/v1/search";
const PLAYBACK_API: &str = "https://music.91q.com/v1/song/tracklink";
const APP_ID: &str = "16073360";
const SECRET: &str = "0b50b02fd0d73a9c4c8c3a781c30845f";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/143.0.0.0 Safari/537.36";

pub struct QianqianSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> QianqianSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for QianqianSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "qianqian"
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
            qualities: Vec::new(),
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_default();
        let mut params = BTreeMap::from([
            ("word", query.query),
            ("type", "1".into()),
            ("pageNo", query.page.to_string()),
            ("pageSize", query.page_size.to_string()),
            ("appid", APP_ID.into()),
            ("timestamp", timestamp),
        ]);
        params.insert("sign", sign(&params));
        let mut url = reqwest::Url::parse(SEARCH_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut().extend_pairs(params.iter());
        let response = self
            .client
            .get_with_headers(
                url.as_str(),
                &[
                    ("User-Agent", USER_AGENT.into()),
                    ("Referer", "https://music.91q.com/player".into()),
                    ("Cookie", self.cookie.value()),
                ],
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("qianqian returned HTTP {}", response.status),
                response.status == 429 || response.status >= 500,
            ));
        }
        parse_search(&response.body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        if song.source != self.id() || song.scope != "song" || song.site_id.trim().is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid Qianqian song reference".into(),
                false,
            ));
        }
        let mut last_error = None;
        for rate in ["3000", "320", "128", "64"] {
            match self.resolve_rate(&song, rate).await {
                Ok(resource) => return Ok(resource),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            source_error(
                "SOURCE_PLAYBACK_UNAVAILABLE",
                "Qianqian response contained no playable URL".into(),
                false,
            )
        }))
    }
}

impl<C> QianqianSource<C>
where
    C: HttpClient,
{
    async fn resolve_rate(
        &self,
        song: &SongRef,
        rate: &str,
    ) -> Result<PlayableResource, SourceError> {
        let timestamp = unix_timestamp();
        let mut params = BTreeMap::from([
            ("TSID", song.site_id.clone()),
            ("appid", APP_ID.into()),
            ("rate", rate.into()),
            ("timestamp", timestamp),
        ]);
        params.insert("sign", sign(&params));
        let mut url = reqwest::Url::parse(PLAYBACK_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut().extend_pairs(params.iter());
        let response = self
            .client
            .get_with_headers(
                url.as_str(),
                &[
                    ("User-Agent", USER_AGENT.into()),
                    ("Referer", "https://music.91q.com/player".into()),
                    ("Cookie", self.cookie.value()),
                ],
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("qianqian returned HTTP {}", response.status),
                response.status == 429 || response.status >= 500,
            ));
        }
        let playback = parse_playback(&response.body)?;
        Ok(PlayableResource {
            song: song.clone(),
            url: playback.url,
            quality: if playback.format.is_empty() {
                rate.into()
            } else {
                playback.format
            },
            duration_ms: (playback.duration > 0)
                .then(|| (playback.duration as u64).saturating_mul(1000)),
            expires_at: None,
        })
    }
}

fn unix_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_default()
}

fn sign(params: &BTreeMap<&str, String>) -> String {
    let source = params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{:x}", md5::compute(format!("{source}{SECRET}")))
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    let first = body
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace());
    if first.is_none() || first == Some(b'<') {
        return Err(source_error(
            "SOURCE_UPSTREAM_UNAVAILABLE",
            "qianqian returned an empty or non-JSON response".into(),
            true,
        ));
    }
    let payload: SearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    if !payload.state && payload.errno != 0 {
        return Err(source_error(
            "SOURCE_API_ERROR",
            format!(
                "qianqian returned code {}: {}",
                payload.errno, payload.errmsg
            ),
            false,
        ));
    }
    let items = payload
        .data
        .tracks
        .into_iter()
        .filter(|track| track.is_vip == 0)
        .map(|track| Song {
            reference: SongRef {
                source: "qianqian".into(),
                scope: "song".into(),
                site_id: track.tsid,
            },
            title: track.title,
            artists: track
                .artists
                .into_iter()
                .filter(|artist| artist.artist_type == 38)
                .map(|artist| Artist {
                    id: None,
                    name: artist.name,
                })
                .collect(),
            album: Some(AlbumRef {
                id: normalize_album_id(&track.album_id),
                name: track.album_title,
                source: Some("qianqian".into()),
            }),
            duration_ms: Some((track.duration as u64).saturating_mul(1000)),
            cover_url: (!track.pic.is_empty()).then_some(track.pic),
        })
        .collect();
    Ok(SearchResult {
        items,
        total: None,
        page,
        page_size,
    })
}

fn parse_playback(body: &[u8]) -> Result<Playback, SourceError> {
    let payload: PlaybackResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let url = if is_http_url(&payload.data.path) {
        payload.data.path
    } else if is_http_url(&payload.data.trial.path) {
        payload.data.trial.path
    } else {
        return Err(source_error(
            "SOURCE_PLAYBACK_UNAVAILABLE",
            "Qianqian response contained no playable URL".into(),
            false,
        ));
    };
    Ok(Playback {
        url,
        format: payload.data.format,
        duration: payload.data.duration,
    })
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

fn normalize_album_id(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() >= 2 && matches!(value.as_bytes()[0], b'p' | b'P'))
        .then(|| format!("P{}", &value[1..]))
}

fn source_error(code: &str, message: String, retryable: bool) -> SourceError {
    SourceError {
        code: code.into(),
        message,
        retryable,
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    state: bool,
    #[serde(default)]
    errno: i32,
    #[serde(default)]
    errmsg: String,
    data: SearchData,
}

#[derive(Deserialize)]
struct SearchData {
    #[serde(rename = "typeTrack", default)]
    tracks: Vec<SearchTrack>,
}

#[derive(Deserialize)]
struct SearchTrack {
    #[serde(rename = "TSID")]
    tsid: String,
    #[serde(default)]
    title: String,
    #[serde(rename = "albumTitle", default)]
    album_title: String,
    #[serde(rename = "albumAssetCode", default)]
    album_id: String,
    #[serde(default)]
    pic: String,
    #[serde(default)]
    duration: u32,
    #[serde(rename = "artist", default)]
    artists: Vec<SearchArtist>,
    #[serde(rename = "isVip", default)]
    is_vip: u8,
}

#[derive(Deserialize)]
struct SearchArtist {
    #[serde(default)]
    name: String,
    #[serde(rename = "artistType", default)]
    artist_type: i32,
}

#[derive(Deserialize)]
struct PlaybackResponse {
    data: PlaybackData,
}

#[derive(Deserialize)]
struct PlaybackData {
    #[serde(default)]
    path: String,
    #[serde(default)]
    format: String,
    #[serde(default)]
    duration: u32,
    #[serde(rename = "trail_audio_info", default)]
    trial: TrialAudio,
}

#[derive(Default, Deserialize)]
struct TrialAudio {
    #[serde(default)]
    path: String,
}

struct Playback {
    url: String,
    format: String,
    duration: u32,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{parse_playback, parse_search, sign, QianqianSource, APP_ID};
    use crate::domain::source::{MusicSource, SearchQuery};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    #[test]
    fn signs_fixed_parameters() {
        let params = BTreeMap::from([
            ("appid", APP_ID.into()),
            ("pageNo", "1".into()),
            ("pageSize", "10".into()),
            ("timestamp", "1700000000".into()),
            ("type", "1".into()),
            ("word", "test".into()),
        ]);
        assert_eq!(sign(&params), "d838b89c2f6ba6919bd1b4d0ece2d361");
    }

    #[test]
    fn parses_fixture_and_filters_vip_track() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/qianqian/search.json"),
            1,
            10,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].reference.site_id, "T100");
        assert_eq!(
            result.items[0]
                .album
                .as_ref()
                .and_then(|album| album.id.as_deref()),
            Some("P200")
        );
    }

    #[test]
    fn parses_trial_playback_fallback() {
        let playback = parse_playback(include_bytes!(
            "../../../../tests/fixtures/qianqian/playback.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(playback.url, "https://example.test/trial.mp3");
        assert_eq!(playback.format, "mp3");
        assert_eq!(playback.duration, 30);
    }

    #[tokio::test]
    #[ignore = "requires live Qianqian service"]
    async fn resolves_live_resource() {
        let source = QianqianSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let search_result = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 10,
                sources: vec!["qianqian".into()],
            })
            .await;
        let search = match search_result {
            Ok(search) => search,
            Err(error) => panic!("live search failed: {error:?}"),
        };
        let result = source.resolve_song(search.items[0].reference.clone()).await;
        assert!(result.is_ok(), "{result:?}");
        assert!(result
            .unwrap_or_else(|_| unreachable!())
            .url
            .starts_with("http"));
    }

    #[tokio::test]
    #[ignore = "requires live Qianqian service"]
    async fn searches_live_service() {
        let source = QianqianSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 10,
                sources: vec!["qianqian".into()],
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }
}
