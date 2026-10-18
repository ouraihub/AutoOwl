use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::media::PlayableResource;
use crate::domain::source::{
    Artist, MusicSource, SearchQuery, SearchResult, Song, SongRef, SourceCapabilities, SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::HttpClient;

const SEARCH_API: &str = "http://search.5sing.kugou.com/home/json";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/134.0.0.0 Safari/537.36";

pub struct FivesingSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> FivesingSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for FivesingSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "fivesing"
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
            qualities: vec!["sq".into(), "hq".into(), "lq".into()],
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let mut url = reqwest::Url::parse(SEARCH_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("keyword", &query.query)
            .append_pair("sort", "1")
            .append_pair("page", &query.page.to_string())
            .append_pair("filter", "0")
            .append_pair("type", "0");
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
                format!("fivesing returned HTTP {}", response.status),
                response.status == 429 || response.status >= 500,
            ));
        }
        parse_search(&response.body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        if song.source != self.id() || song.scope != "song" {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid FiveSing song reference".into(),
                false,
            ));
        }
        let (song_id, song_type) = song.site_id.split_once('|').ok_or_else(|| {
            source_error(
                "INVALID_ARGUMENT",
                "FiveSing song id must contain type".into(),
                false,
            )
        })?;
        if song_id.is_empty() || !matches!(song_type, "yc" | "fc" | "bz") {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid FiveSing song id or type".into(),
                false,
            ));
        }
        let mut url = reqwest::Url::parse("http://mobileapi.5sing.kugou.com/song/getSongUrl")
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("songid", song_id)
            .append_pair("songtype", song_type);
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
                format!("fivesing returned HTTP {}", response.status),
                response.status == 429 || response.status >= 500,
            ));
        }
        parse_playable(&response.body, song)
    }
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    let payload: SearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let items = payload
        .list
        .into_iter()
        .map(|item| Song {
            reference: SongRef {
                source: "fivesing".into(),
                scope: "song".into(),
                site_id: format!("{}|{}", item.song_id, item.type_name),
            },
            title: clean_text(&item.song_name),
            artists: vec![Artist {
                id: None,
                name: clean_text(&item.singer),
            }],
            album: None,
            duration_ms: (item.song_size > 0)
                .then(|| ((item.song_size as u64).saturating_mul(8) / 320_000) * 1000),
            cover_url: None,
        })
        .collect();
    Ok(SearchResult {
        items,
        total: None,
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
        .replace("&quot;", "\"")
        .trim()
        .to_owned()
}

fn parse_playable(body: &[u8], song: SongRef) -> Result<PlayableResource, SourceError> {
    let payload: PlaybackResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    if payload.code != 1000 {
        return Err(source_error(
            "SOURCE_API_ERROR",
            format!("FiveSing returned code {}", payload.code),
            false,
        ));
    }
    let candidates = [
        ("sq", payload.data.sq_url),
        ("sq", payload.data.sq_url_backup),
        ("hq", payload.data.hq_url),
        ("hq", payload.data.hq_url_backup),
        ("lq", payload.data.lq_url),
        ("lq", payload.data.lq_url_backup),
    ];
    let (quality, url) = candidates
        .into_iter()
        .find(|(_, url)| !url.trim().is_empty())
        .ok_or_else(|| {
            source_error(
                "SOURCE_PLAYBACK_UNAVAILABLE",
                "FiveSing track has no playable stream".into(),
                false,
            )
        })?;
    Ok(PlayableResource {
        song,
        url,
        quality: quality.into(),
        duration_ms: None,
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
struct SearchResponse {
    #[serde(default)]
    list: Vec<SearchSong>,
}

#[derive(Deserialize)]
struct SearchSong {
    #[serde(rename = "songId")]
    song_id: i64,
    #[serde(rename = "songName", default)]
    song_name: String,
    #[serde(default)]
    singer: String,
    #[serde(rename = "songSize", default)]
    song_size: i64,
    #[serde(rename = "typeEname", default)]
    type_name: String,
}

#[derive(Deserialize)]
struct PlaybackResponse {
    code: i32,
    data: PlaybackData,
}

#[derive(Deserialize)]
struct PlaybackData {
    #[serde(rename = "squrl", default)]
    sq_url: String,
    #[serde(rename = "squrl_backup", default)]
    sq_url_backup: String,
    #[serde(rename = "hqurl", default)]
    hq_url: String,
    #[serde(rename = "hqurl_backup", default)]
    hq_url_backup: String,
    #[serde(rename = "lqurl", default)]
    lq_url: String,
    #[serde(rename = "lqurl_backup", default)]
    lq_url_backup: String,
}

#[cfg(test)]
mod tests {
    use super::{clean_text, parse_playable, parse_search, FivesingSource};
    use crate::domain::source::{MusicSource, SearchQuery, SongRef};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    #[test]
    fn parses_fixture_and_cleans_highlights() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/fivesing/search.json"),
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.items[0].reference.site_id, "12345|yc");
        assert_eq!(result.items[0].title, "Fixture & Song");
        assert_eq!(result.items[0].duration_ms, Some(200_000));
        assert_eq!(clean_text("<em>A</em> &amp; B"), "A & B");
    }

    #[test]
    fn resolves_highest_fixture_stream() {
        let resource = parse_playable(
            include_bytes!("../../../../tests/fixtures/fivesing/playback.json"),
            SongRef {
                source: "fivesing".into(),
                scope: "song".into(),
                site_id: "12345|yc".into(),
            },
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(resource.quality, "sq");
        assert_eq!(resource.url, "https://example.test/song.flac");
    }

    #[tokio::test]
    #[ignore = "requires live FiveSing service"]
    async fn resolves_live_resource() {
        let source = FivesingSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let search = source
            .search_songs(SearchQuery {
                query: "古风".into(),
                page: 1,
                page_size: 20,
                sources: vec!["fivesing".into()],
            })
            .await
            .unwrap_or_else(|_| unreachable!());
        let result = source.resolve_song(search.items[0].reference.clone()).await;
        assert!(result.is_ok(), "{result:?}");
        assert!(result
            .unwrap_or_else(|_| unreachable!())
            .url
            .starts_with("http"));
    }

    #[tokio::test]
    #[ignore = "requires live FiveSing service"]
    async fn searches_live_service() {
        let source = FivesingSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "古风".into(),
                page: 1,
                page_size: 20,
                sources: vec!["fivesing".into()],
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }
}
