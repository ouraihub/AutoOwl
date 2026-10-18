use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::media::PlayableResource;
use crate::domain::source::{
    AlbumRef, Artist, MusicSource, SearchQuery, SearchResult, Song, SongRef, SourceCapabilities,
    SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::HttpClient;

const HOME_URL: &str = "https://music.apple.com";
const API_URL: &str = "https://amp-api.music.apple.com/v1/catalog/us/search";
const SONG_API: &str = "https://amp-api.music.apple.com/v1/catalog/us/songs";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/134.0.0.0 Safari/537.36";

pub struct AppleSource<C> {
    client: C,
    media_user_token: SourceCookie,
}

impl<C> AppleSource<C> {
    pub fn new(client: C, media_user_token: SourceCookie) -> Self {
        Self {
            client,
            media_user_token,
        }
    }
}

#[async_trait]
impl<C> MusicSource for AppleSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "apple"
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
            qualities: Vec::new(),
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let token = self.fetch_token().await?;
        let mut url = reqwest::Url::parse(API_URL)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("term", &query.query)
            .append_pair("types", "songs")
            .append_pair("limit", &query.page_size.min(25).to_string());
        let headers = self.catalog_headers(&token);
        let response = self
            .client
            .get_with_headers(url.as_str(), &headers)
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        ensure_success(response.status, "catalog")?;
        parse_search(&response.body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        if song.source != self.id() || song.scope != "song" || song.site_id.trim().is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid Apple Music song reference".into(),
                false,
            ));
        }
        let token = self.fetch_token().await?;
        let url = format!("{SONG_API}/{}", song.site_id);
        let response = self
            .client
            .get_with_headers(&url, &self.catalog_headers(&token))
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        ensure_success(response.status, "song")?;
        let preview = parse_playback(&response.body)?;
        Ok(PlayableResource {
            song,
            url: preview.url,
            quality: "preview".into(),
            duration_ms: (preview.duration_ms > 0).then_some(preview.duration_ms),
            expires_at: None,
        })
    }
}

impl<C> AppleSource<C>
where
    C: HttpClient,
{
    fn catalog_headers(&self, token: &str) -> Vec<(&str, String)> {
        let mut headers = vec![
            ("Authorization", format!("Bearer {token}")),
            ("Origin", HOME_URL.into()),
        ];
        let media_user_token = self.media_user_token.value();
        if !media_user_token.is_empty() {
            headers.push(("Cookie", format!("media-user-token={media_user_token}")));
        }
        headers
    }

    async fn fetch_token(&self) -> Result<String, SourceError> {
        let homepage = self
            .client
            .get_with_headers(HOME_URL, &[("User-Agent", USER_AGENT.into())])
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        ensure_success(homepage.status, "homepage")?;
        let html = String::from_utf8_lossy(&homepage.body);
        let assets = extract_script_assets(&html);
        if assets.is_empty() {
            return Err(source_error(
                "SOURCE_RESPONSE_INVALID",
                "apple script assets not found".into(),
                false,
            ));
        }
        for asset in assets.into_iter().take(16) {
            let script_url = format!("{HOME_URL}/{asset}");
            let script = self
                .client
                .get_with_headers(&script_url, &[("User-Agent", USER_AGENT.into())])
                .await
                .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
            if !(200..300).contains(&script.status) {
                continue;
            }
            if let Some(token) = extract_token(&String::from_utf8_lossy(&script.body)) {
                return Ok(token);
            }
        }
        Err(source_error(
            "SOURCE_RESPONSE_INVALID",
            "apple catalog token not found".into(),
            false,
        ))
    }
}

fn ensure_success(status: u16, endpoint: &str) -> Result<(), SourceError> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(source_error(
            "SOURCE_HTTP_ERROR",
            format!("apple {endpoint} returned HTTP {status}"),
            status == 429 || status >= 500,
        ))
    }
}

fn extract_script_assets(html: &str) -> Vec<String> {
    let mut assets = Vec::new();
    let mut remaining = html;
    while let Some(start) = remaining.find("assets/") {
        let tail = &remaining[start..];
        let Some(end) = tail.find(".js").map(|index| index + 3) else {
            break;
        };
        let asset = &tail[..end];
        if !asset.contains(['\"', '\'', '<', '>']) && !assets.iter().any(|item| item == asset) {
            assets.push(asset.to_owned());
        }
        remaining = &tail[end..];
    }
    assets.sort_by_key(|asset| !asset.contains("index-legacy"));
    assets
}

fn extract_token(script: &str) -> Option<String> {
    let start = script.find("eyJh")?;
    let token = script[start..]
        .split(['\"', '\'', '\\'])
        .next()
        .unwrap_or_default();
    (token.matches('.').count() == 2).then(|| token.to_owned())
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    let payload: SearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let items = payload
        .results
        .songs
        .data
        .into_iter()
        .map(|resource| {
            let attributes = resource.attributes;
            Song {
                reference: SongRef {
                    source: "apple".into(),
                    scope: "song".into(),
                    site_id: resource.id,
                },
                title: attributes.name,
                artists: vec![Artist {
                    id: None,
                    name: attributes.artist_name,
                }],
                album: Some(AlbumRef {
                    id: None,
                    name: attributes.album_name,
                    source: Some("apple".into()),
                }),
                duration_ms: Some(attributes.duration_in_millis),
                cover_url: (!attributes.artwork.url.is_empty()).then(|| {
                    attributes
                        .artwork
                        .url
                        .replace("{w}", "600")
                        .replace("{h}", "600")
                }),
            }
        })
        .collect();
    Ok(SearchResult {
        items,
        total: None,
        page,
        page_size,
    })
}

fn parse_playback(body: &[u8]) -> Result<Preview, SourceError> {
    let payload: SongResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let attributes = payload.data.into_iter().next().ok_or_else(|| {
        source_error(
            "SOURCE_PLAYBACK_UNAVAILABLE",
            "Apple Music song detail was not found".into(),
            false,
        )
    })?;
    let url = attributes
        .attributes
        .previews
        .into_iter()
        .map(|preview| preview.url)
        .find(|url| is_http_url(url))
        .ok_or_else(|| {
            source_error(
                "SOURCE_PLAYBACK_UNAVAILABLE",
                "Apple Music song has no public preview URL".into(),
                false,
            )
        })?;
    Ok(Preview {
        url,
        duration_ms: attributes.attributes.duration_in_millis,
    })
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
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
    results: SearchResults,
}

#[derive(Deserialize)]
struct SearchResults {
    songs: SearchSongs,
}

#[derive(Deserialize)]
struct SearchSongs {
    #[serde(default)]
    data: Vec<SearchResource>,
}

#[derive(Deserialize)]
struct SearchResource {
    id: String,
    attributes: SongAttributes,
}

#[derive(Deserialize)]
struct SongAttributes {
    #[serde(default)]
    name: String,
    #[serde(rename = "artistName", default)]
    artist_name: String,
    #[serde(rename = "albumName", default)]
    album_name: String,
    #[serde(rename = "durationInMillis", default)]
    duration_in_millis: u64,
    #[serde(default)]
    artwork: Artwork,
    #[serde(default)]
    previews: Vec<SongPreview>,
}

#[derive(Deserialize)]
struct SongPreview {
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
struct SongResponse {
    #[serde(default)]
    data: Vec<SearchResource>,
}

struct Preview {
    url: String,
    duration_ms: u64,
}

#[derive(Default, Deserialize)]
struct Artwork {
    #[serde(default)]
    url: String,
}

#[cfg(test)]
mod tests {
    use super::{extract_script_assets, extract_token, parse_playback, parse_search, AppleSource};
    use crate::domain::source::{MusicSource, SearchQuery};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    #[test]
    fn extracts_asset_and_token() {
        assert_eq!(
            extract_script_assets(
                r#"<script src="/assets/runtime.js"></script><script src="/assets/index-legacy-abc.js"></script>"#
            ),
            vec!["assets/index-legacy-abc.js", "assets/runtime.js"]
        );
        assert_eq!(
            extract_token(r#"token="eyJheader.payload.signature""#).as_deref(),
            Some("eyJheader.payload.signature")
        );
    }

    #[test]
    fn parses_catalog_fixture() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/apple/search.json"),
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.items[0].reference.site_id, "APPLE100");
        assert_eq!(result.items[0].duration_ms, Some(205_000));
        assert_eq!(
            result.items[0].cover_url.as_deref(),
            Some("https://example.test/600x600.jpg")
        );
    }

    #[test]
    fn parses_public_preview_fixture() {
        let preview = parse_playback(include_bytes!(
            "../../../../tests/fixtures/apple/playback.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(preview.url, "https://example.test/preview.m4a");
        assert_eq!(preview.duration_ms, 205_000);
    }

    #[tokio::test]
    #[ignore = "requires live Apple Music service"]
    async fn resolves_live_preview() {
        let source = AppleSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let search = source
            .search_songs(SearchQuery {
                query: "Taylor Swift".into(),
                page: 1,
                page_size: 5,
                sources: vec!["apple".into()],
            })
            .await;
        let search = match search {
            Ok(search) => search,
            Err(error) => panic!("live search failed: {error:?}"),
        };
        let result = source.resolve_song(search.items[0].reference.clone()).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(result.unwrap_or_else(|_| unreachable!()).quality, "preview");
    }

    #[tokio::test]
    #[ignore = "requires live Apple Music service"]
    async fn searches_live_service() {
        let source = AppleSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "Taylor Swift".into(),
                page: 1,
                page_size: 5,
                sources: vec!["apple".into()],
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }
}
