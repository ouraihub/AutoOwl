use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::media::PlayableResource;
use crate::domain::source::{
    AlbumRef, Artist, MusicSource, SearchQuery, SearchResult, Song, SongRef, SourceCapabilities,
    SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::{adapter_headers, HttpClient};

const SEARCH_API: &str = "http://songsearch.kugou.com/song_search_v2";
const PLAYBACK_API: &str = "http://m.kugou.com/app/i/getSongInfo.php";
/// Android agent kugou's search endpoint expects.
const SEARCH_USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 10; SM-G981B) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/80.0.3987.162 Mobile Safari/537.36";
/// iPhone agent the mobile playback endpoint expects.
const PLAYBACK_USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 13_2_3 like Mac OS X) \
AppleWebKit/605.1.15 (KHTML, like Gecko) Version/13.0.3 Mobile/15E148 Safari/604.1";

pub struct KugouSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> KugouSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for KugouSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "kugou"
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
            qualities: vec!["standard".into()],
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let mut url = reqwest::Url::parse(SEARCH_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis().to_string())
            .unwrap_or_default();
        url.query_pairs_mut()
            .append_pair("keyword", &query.query)
            .append_pair("platform", "WebFilter")
            .append_pair("format", "json")
            .append_pair("page", &query.page.to_string())
            .append_pair("pagesize", &query.page_size.to_string())
            .append_pair("userid", "-1")
            .append_pair("tag", "em")
            .append_pair("filter", "2")
            .append_pair("iscorrection", "1")
            .append_pair("privilege_filter", "0")
            .append_pair("_", &timestamp);
        let response = self
            .client
            .get_with_headers(
                url.as_str(),
                &adapter_headers(SEARCH_USER_AGENT, None, &self.cookie.value()),
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("kugou returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        parse_search(&response.body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        if song.source != self.id() || song.scope != "song" || song.site_id.trim().is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid Kugou song reference".into(),
                false,
            ));
        }
        let mut url = reqwest::Url::parse(PLAYBACK_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("cmd", "playInfo")
            .append_pair("hash", &song.site_id);
        let response = self
            .client
            .get_with_headers(
                url.as_str(),
                &adapter_headers(
                    PLAYBACK_USER_AGENT,
                    Some("http://m.kugou.com/"),
                    &self.cookie.value(),
                ),
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("kugou returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        let (playback_url, quality, duration_ms) = parse_playback(&response.body)?;
        Ok(PlayableResource {
            song,
            url: playback_url,
            quality,
            duration_ms,
            expires_at: None,
        })
    }
}

fn parse_playback(body: &[u8]) -> Result<(String, String, Option<u64>), SourceError> {
    let payload: PlaybackResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    if payload.errcode != 0 {
        return Err(source_error(
            "SOURCE_PLAYBACK_UNAVAILABLE",
            format!("Kugou playback is unavailable, errcode={}", payload.errcode),
            payload.errcode == 1002,
        ));
    }
    let url = payload.url.trim();
    if !url.starts_with("https://") && !url.starts_with("http://") {
        // A paywalled hash answers with errcode 0, an empty url and a human
        // readable reason such as "需要付费"; forward it instead of a generic message.
        let reason = payload.error.trim();
        return Err(source_error(
            "SOURCE_PLAYBACK_UNAVAILABLE",
            if reason.is_empty() {
                "Kugou track has no playable stream".into()
            } else {
                format!("Kugou refused playback: {reason}")
            },
            false,
        ));
    }
    Ok((
        url.into(),
        if payload.bit_rate > 0 {
            format!("{}kbps", payload.bit_rate / 1000)
        } else {
            "standard".into()
        },
        (payload.time_length > 0).then(|| (payload.time_length as u64).saturating_mul(1000)),
    ))
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    let payload: SearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let items = payload
        .data
        .lists
        .into_iter()
        .filter_map(|song| {
            let hash = [
                song.file_hash,
                song.sq_file_hash,
                song.hq_file_hash,
                song.res_file_hash,
                song.trans_param.ogg_320_hash,
                song.trans_param.ogg_128_hash,
            ]
            .into_iter()
            .find(|value| !value.trim().is_empty())?;
            let artists = clean_text(&song.singer_name)
                .split(['、', '/', ','])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|name| Artist {
                    id: None,
                    name: name.into(),
                })
                .collect();
            let album_name = clean_text(&song.album_name);
            Some(Song {
                reference: SongRef {
                    source: "kugou".into(),
                    scope: "song".into(),
                    site_id: hash,
                },
                title: clean_text(&song.song_name),
                artists,
                album: Some(AlbumRef {
                    id: (!song.album_id.is_empty()).then_some(song.album_id),
                    name: album_name,
                    source: Some("kugou".into()),
                }),
                duration_ms: Some((song.duration as u64).saturating_mul(1000)),
                cover_url: (!song.image.is_empty()).then(|| song.image.replace("{size}", "240")),
            })
        })
        .collect();
    Ok(SearchResult {
        items,
        total: payload.data.total,
        page,
        page_size,
    })
}

fn clean_text(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut inside_tag = false;
    for character in value.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => result.push(character),
            _ => {}
        }
    }
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_owned()
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
    data: SearchData,
}

#[derive(Deserialize)]
struct SearchData {
    #[serde(rename = "lists", default)]
    lists: Vec<SearchSong>,
    total: Option<u64>,
}

#[derive(Deserialize)]
struct SearchSong {
    #[serde(rename = "SongName")]
    song_name: String,
    #[serde(rename = "SingerName")]
    singer_name: String,
    #[serde(rename = "AlbumName", default)]
    album_name: String,
    #[serde(rename = "AlbumID", default)]
    album_id: String,
    #[serde(rename = "Duration", default)]
    duration: u32,
    #[serde(rename = "FileHash", default)]
    file_hash: String,
    #[serde(rename = "SQFileHash", default)]
    sq_file_hash: String,
    #[serde(rename = "HQFileHash", default)]
    hq_file_hash: String,
    #[serde(rename = "ResFileHash", default)]
    res_file_hash: String,
    #[serde(rename = "Image", default)]
    image: String,
    #[serde(default)]
    trans_param: SearchTransParam,
}

#[derive(Default, Deserialize)]
struct SearchTransParam {
    #[serde(default)]
    ogg_320_hash: String,
    #[serde(default)]
    ogg_128_hash: String,
}

#[derive(Deserialize)]
struct PlaybackResponse {
    #[serde(default)]
    url: String,
    #[serde(rename = "bitRate", default)]
    bit_rate: u32,
    #[serde(rename = "timeLength", default)]
    time_length: u32,
    #[serde(default)]
    errcode: i32,
    /// Human readable refusal reason, e.g. "需要付费" for paywalled hashes.
    #[serde(default)]
    error: String,
}

#[cfg(test)]
mod tests {
    use super::{clean_text, parse_playback, parse_search, KugouSource};
    use crate::domain::source::{MusicSource, SearchQuery};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    #[test]
    fn parses_search_fixture_and_cleans_highlights() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/kugou/search.json"),
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.items[0].title, "Fixture & Song");
        assert_eq!(result.items[0].reference.site_id, "HASH123");
    }

    #[test]
    fn strips_search_markup() {
        assert_eq!(clean_text("<em>Hello</em> &amp; World"), "Hello & World");
    }

    #[test]
    fn parses_playback_fixture() {
        let resource = parse_playback(include_bytes!(
            "../../../../tests/fixtures/kugou/playback.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(resource.0, "https://example.test/song.mp3");
        assert_eq!(resource.1, "128kbps");
        assert_eq!(resource.2, Some(186_000));
    }

    #[test]
    fn paywalled_playback_forwards_server_reason() {
        const PAYWALLED: &str = r#"{"status": 0, "errcode": 0, "url": "", "error": "需要付费"}"#;
        let error = parse_playback(PAYWALLED.as_bytes()).expect_err("empty url must fail");
        assert_eq!(error.code, "SOURCE_PLAYBACK_UNAVAILABLE");
        assert!(
            error.message.contains("需要付费"),
            "reason must reach the caller, got {}",
            error.message
        );
        assert!(!error.retryable);
    }

    #[tokio::test]
    #[ignore = "requires live Kugou service"]
    async fn resolves_live_resource() {
        let source = KugouSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let search = source
            .search_songs(SearchQuery {
                // Kugou serves 周杰伦 as paid-only across every tier, so use a
                // catalogue with free masters or the test measures licensing
                // rather than the adapter.
                query: "千千阙歌".into(),
                page: 1,
                page_size: 30,
                sources: vec!["kugou".into()],
            })
            .await
            .unwrap_or_else(|error| panic!("live search failed: {error:?}"));
        assert!(!search.items.is_empty(), "kugou search returned no rows");
        let mut last_error = None;
        for item in search.items {
            match source.resolve_song(item.reference).await {
                Ok(resource) => {
                    assert!(resource.url.starts_with("http"));
                    return;
                }
                Err(error) => last_error = Some(error),
            }
        }
        panic!("no live search result resolved: {last_error:?}");
    }

    #[tokio::test]
    #[ignore = "requires live Kugou service"]
    async fn searches_live_service() {
        let source = KugouSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 5,
                sources: vec!["kugou".into()],
            })
            .await;
        assert!(result.is_ok());
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }
}
