use async_trait::async_trait;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::media::PlayableResource;
use crate::domain::source::{
    AlbumRef, Artist, MusicSource, SearchQuery, SearchResult, Song, SongRef, SourceCapabilities,
    SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::HttpClient;

const SEARCH_API: &str = "http://www.kuwo.cn/search/searchMusicBykeyWord";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/134.0.0.0 Safari/537.36";

pub struct KuwoSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> KuwoSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for KuwoSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "kuwo"
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            song_search: true,
            playlist_search: false,
            album_search: false,
            lyrics: false,
            cover: false,
            cookie: true,
            qr_login: false,
            qualities: vec![
                "128kmp3".into(),
                "320kmp3".into(),
                "flac".into(),
                "2000kflac".into(),
            ],
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let mut url = reqwest::Url::parse(SEARCH_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("vipver", "1")
            .append_pair("client", "kt")
            .append_pair("ft", "music")
            .append_pair("cluster", "0")
            .append_pair("strategy", "2012")
            .append_pair("encoding", "utf8")
            .append_pair("rformat", "json")
            .append_pair("mobi", "1")
            .append_pair("issubtitle", "1")
            .append_pair("show_copyright_off", "1")
            .append_pair("pn", &query.page.saturating_sub(1).to_string())
            .append_pair("rn", &query.page_size.to_string())
            .append_pair("all", &query.query);
        let response = self
            .client
            .get_with_headers(
                url.as_str(),
                &[
                    ("User-Agent", USER_AGENT.into()),
                    ("Cookie", self.cookie.value()),
                ],
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("kuwo returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        parse_search(&response.body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        if song.source != self.id() || song.scope != "song" || song.site_id.trim().is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid Kuwo song reference".into(),
                false,
            ));
        }
        let request_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_default();
        for quality in ["128kmp3", "320kmp3", "flac", "2000kflac"] {
            let mut url = reqwest::Url::parse("https://mobi.kuwo.cn/mobi.s")
                .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
            url.query_pairs_mut()
                .append_pair("f", "web")
                .append_pair("source", "kwplayercar_ar_6.0.0.9_B_jiakong_vh.apk")
                .append_pair("from", "PC")
                .append_pair("type", "convert_url_with_sign")
                .append_pair("br", quality)
                .append_pair("rid", &song.site_id)
                .append_pair("user", &format!("C_APK_guanwang_{request_id}"));
            let Ok(response) = self
                .client
                .get_with_headers(
                    url.as_str(),
                    &[
                        ("User-Agent", USER_AGENT.into()),
                        ("Cookie", self.cookie.value()),
                    ],
                )
                .await
            else {
                continue;
            };
            if let Some((url, actual_quality)) = parse_primary_playable(&response.body) {
                return Ok(PlayableResource {
                    song,
                    url,
                    quality: actual_quality.unwrap_or_else(|| quality.into()),
                    duration_ms: None,
                    expires_at: None,
                });
            }
        }
        self.resolve_fallback(song).await
    }
}

impl<C> KuwoSource<C>
where
    C: HttpClient,
{
    async fn resolve_fallback(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        let mut url = reqwest::Url::parse("http://www.kuwo.cn/api/v1/www/music/playUrl")
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("mid", &song.site_id)
            .append_pair("type", "music")
            .append_pair("httpsStatus", "1");
        let stored = self.cookie.value();
        let cookie = if stored.contains("kw_token") {
            stored
        } else {
            format!("{stored}; kw_token=secret_token")
        };
        let response = self
            .client
            .get_with_headers(
                url.as_str(),
                &[
                    ("User-Agent", USER_AGENT.into()),
                    ("Cookie", cookie),
                    ("Secret", "kuwo_web_secret".into()),
                ],
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        let payload: FallbackPlaybackResponse = serde_json::from_slice(&response.body)
            .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
        let Some(url) = normalize_playback_url(&payload.data.url) else {
            return Err(source_error(
                "SOURCE_PLAYBACK_UNAVAILABLE",
                "Kuwo track has no playable stream".into(),
                false,
            ));
        };
        Ok(PlayableResource {
            song,
            url,
            quality: "fallback".into(),
            duration_ms: None,
            expires_at: None,
        })
    }
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    let payload: SearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let items = payload
        .songs
        .into_iter()
        .filter(|song| song.bit_switch != 0)
        .map(|song| Song {
            reference: SongRef {
                source: "kuwo".into(),
                scope: "song".into(),
                site_id: song.music_rid.trim_start_matches("MUSIC_").to_owned(),
            },
            title: song.song_name,
            artists: song
                .artist
                .split(['&', '/', '、'])
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| Artist {
                    id: None,
                    name: name.into(),
                })
                .collect(),
            album: Some(AlbumRef {
                id: None,
                name: song.album,
                source: Some("kuwo".into()),
            }),
            duration_ms: song
                .duration
                .parse::<u64>()
                .ok()
                .map(|seconds| seconds.saturating_mul(1000)),
            cover_url: (!song.cover_url.is_empty()).then_some(song.cover_url),
        })
        .collect();
    Ok(SearchResult {
        items,
        total: payload.total,
        page,
        page_size,
    })
}

fn parse_primary_playable(body: &[u8]) -> Option<(String, Option<String>)> {
    let payload: PrimaryPlaybackResponse = serde_json::from_slice(body).ok()?;
    let url = normalize_playback_url(&payload.data.url)?;
    let quality = if !payload.data.format.is_empty() && payload.data.bitrate > 0 {
        Some(format!("{}-{}", payload.data.format, payload.data.bitrate))
    } else {
        None
    };
    Some((url, quality))
}

fn normalize_playback_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with("https://") || value.starts_with("http://") {
        Some(value.to_owned())
    } else if value.starts_with("//") {
        Some(format!("https:{value}"))
    } else {
        None
    }
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
    #[serde(rename = "abslist", default)]
    songs: Vec<SearchSong>,
    #[serde(
        rename = "TOTAL",
        default,
        deserialize_with = "deserialize_optional_u64"
    )]
    total: Option<u64>,
}

#[derive(Deserialize)]
struct SearchSong {
    #[serde(rename = "MUSICRID")]
    music_rid: String,
    #[serde(rename = "SONGNAME")]
    song_name: String,
    #[serde(rename = "ARTIST", default)]
    artist: String,
    #[serde(rename = "ALBUM", default)]
    album: String,
    #[serde(rename = "DURATION", default)]
    duration: String,
    #[serde(rename = "hts_MVPIC", default)]
    cover_url: String,
    #[serde(rename = "bitSwitch", default)]
    bit_switch: i32,
}

#[derive(Deserialize)]
struct PrimaryPlaybackResponse {
    data: PrimaryPlaybackData,
}

#[derive(Deserialize)]
struct PrimaryPlaybackData {
    #[serde(default)]
    url: String,
    #[serde(default)]
    bitrate: u32,
    #[serde(default)]
    format: String,
}

#[derive(Deserialize)]
struct FallbackPlaybackResponse {
    #[serde(default)]
    data: FallbackPlaybackData,
}

#[derive(Default, Deserialize)]
struct FallbackPlaybackData {
    #[serde(default)]
    url: String,
}

fn deserialize_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_primary_playable, parse_search, KuwoSource};
    use crate::domain::source::{MusicSource, SearchQuery};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    #[test]
    fn parses_search_fixture_and_filters_unplayable_songs() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/kuwo/search.json"),
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.total, Some(2));
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].reference.site_id, "12345");
        assert_eq!(result.items[0].duration_ms, Some(186_000));
    }

    #[test]
    fn parses_primary_playback_fixture() {
        let resource = parse_primary_playable(include_bytes!(
            "../../../../tests/fixtures/kuwo/playback.json"
        ))
        .unwrap_or_else(|| unreachable!());
        assert_eq!(resource.0, "https://example.test/song.mp3");
        assert_eq!(resource.1.as_deref(), Some("mp3-128"));
    }

    #[tokio::test]
    #[ignore = "requires live Kuwo service"]
    async fn resolves_live_resource() {
        let source = KuwoSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let search = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 5,
                sources: vec!["kuwo".into()],
            })
            .await
            .unwrap_or_else(|_| unreachable!());
        let result = source.resolve_song(search.items[0].reference.clone()).await;
        assert!(result.is_ok(), "{result:?}");
        let url = result.unwrap_or_else(|_| unreachable!()).url;
        assert!(url.starts_with("http"), "unexpected Kuwo URL: {url}");
    }

    #[tokio::test]
    #[ignore = "requires live Kuwo service"]
    async fn searches_live_service() {
        let source = KuwoSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 5,
                sources: vec!["kuwo".into()],
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }
}
