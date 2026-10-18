use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::domain::media::PlayableResource;
use crate::domain::source::{
    AlbumRef, Artist, MusicSource, SearchQuery, SearchResult, Song, SongRef, SourceCapabilities,
    SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::HttpClient;

const SEARCH_API: &str = "https://api.qishui.com/luna/pc/search/track";
const PLAYBACK_API: &str = "https://api.qishui.com/luna/pc/track_v2";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/134.0.0.0 Safari/537.36";

pub struct SodaSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> SodaSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for SodaSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "soda"
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
        let mut url = reqwest::Url::parse(SEARCH_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        let cursor = query.page.saturating_sub(1).saturating_mul(query.page_size);
        url.query_pairs_mut()
            .append_pair("q", &query.query)
            .append_pair("cursor", &cursor.to_string())
            .append_pair("search_method", "input")
            .append_pair("aid", "386088")
            .append_pair("device_platform", "web")
            .append_pair("channel", "pc_web");
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
                format!("soda returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        parse_search(&response.body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        if song.source != self.id() || song.scope != "song" || song.site_id.trim().is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid Soda song reference".into(),
                false,
            ));
        }
        let mut url = reqwest::Url::parse(PLAYBACK_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("track_id", &song.site_id)
            .append_pair("media_type", "track")
            .append_pair("aid", "386088")
            .append_pair("device_platform", "web")
            .append_pair("channel", "pc_web");
        let response = self.request(url.as_str()).await?;
        let discovery = parse_track_v2(&response.body)?;
        let stream = if let Some(stream) = discovery.stream {
            stream
        } else if let Some(player_info_url) = discovery.player_info_url {
            let response = self.request(&player_info_url).await?;
            parse_player_info(&response.body)?
        } else {
            return Err(source_error(
                "SOURCE_PLAYBACK_UNAVAILABLE",
                "Soda response contained no audio stream".into(),
                false,
            ));
        };
        if !stream.play_auth.is_empty() {
            return Err(source_error(
                "SOURCE_PLAYBACK_ENCRYPTED",
                "Soda stream requires play_auth decryption before playback".into(),
                false,
            ));
        }
        Ok(PlayableResource {
            song,
            url: stream.url,
            quality: stream.quality,
            duration_ms: (stream.duration > 0).then(|| stream.duration.saturating_mul(1000)),
            expires_at: None,
        })
    }
}

impl<C> SodaSource<C>
where
    C: HttpClient,
{
    async fn request(
        &self,
        url: &str,
    ) -> Result<crate::infrastructure::http::HttpResponse, SourceError> {
        let response = self
            .client
            .get_with_headers(
                url,
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
                format!("soda returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        Ok(response)
    }
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err(source_error(
            "SOURCE_EMPTY_RESPONSE",
            "soda returned an empty response".into(),
            true,
        ));
    }
    let payload: SearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let items = payload
        .result_groups
        .into_iter()
        .next()
        .map(|group| {
            group
                .data
                .into_iter()
                .filter_map(|item| to_song(item.entity.track))
                .collect()
        })
        .unwrap_or_default();
    Ok(SearchResult {
        items,
        total: None,
        page,
        page_size,
    })
}

fn parse_track_v2(body: &[u8]) -> Result<Discovery, SourceError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err(source_error(
            "SOURCE_EMPTY_RESPONSE",
            "soda returned an empty playback response".into(),
            true,
        ));
    }
    let payload: TrackV2Response = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    if payload.status_code != 0 {
        return Err(source_error(
            "SOURCE_API_ERROR",
            format!("soda returned code {}", payload.status_code),
            false,
        ));
    }
    let stream = find_stream(&payload.track_player.video_model, "");
    let player_info_url = payload
        .track_player
        .url_player_info
        .filter(|url| is_http_url(url));
    Ok(Discovery {
        stream,
        player_info_url,
    })
}

fn parse_player_info(body: &[u8]) -> Result<Stream, SourceError> {
    let payload: PlayerInfoResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    payload
        .result
        .data
        .play_info_list
        .into_iter()
        .filter_map(|item| {
            let url = [item.main_url, item.backup_url]
                .into_iter()
                .find(|url| is_http_url(url))?;
            Some(Stream {
                url,
                play_auth: item.play_auth,
                quality: first_non_empty([item.quality, item.format]),
                bitrate: item.bitrate,
                size: item.size,
                duration: item.duration as u64,
            })
        })
        .max_by_key(|stream| (stream.bitrate, stream.size))
        .ok_or_else(|| {
            source_error(
                "SOURCE_PLAYBACK_UNAVAILABLE",
                "Soda player info contained no audio stream".into(),
                false,
            )
        })
}

fn find_stream(value: &Value, inherited_auth: &str) -> Option<Stream> {
    match value {
        Value::Object(values) => {
            let auth = object_string(values, &["play_auth", "PlayAuth", "spade_a"])
                .or_else(|| {
                    ["encrypt_info", "EncryptInfo", "encryptInfo"]
                        .iter()
                        .find_map(|key| values.get(*key)?.as_object())
                        .and_then(|child| {
                            object_string(child, &["spade_a", "play_auth", "PlayAuth"])
                        })
                })
                .unwrap_or_else(|| inherited_auth.to_owned());
            let current = object_string(values, &["main_play_url", "MainPlayUrl", "url", "URL"])
                .filter(|url| is_http_url(url))
                .map(|url| Stream {
                    url,
                    play_auth: auth.clone(),
                    quality: object_string(values, &["quality", "Quality", "format", "Format"])
                        .unwrap_or_else(|| "unknown".into()),
                    bitrate: object_u64(values, &["bitrate", "Bitrate", "br"]),
                    size: object_u64(values, &["size", "Size"]),
                    duration: object_u64(values, &["duration", "Duration"]),
                });
            values
                .values()
                .filter_map(|child| find_stream(child, &auth))
                .chain(current)
                .max_by_key(|stream| (stream.bitrate, stream.size))
        }
        Value::Array(values) => values
            .iter()
            .filter_map(|child| find_stream(child, inherited_auth))
            .max_by_key(|stream| (stream.bitrate, stream.size)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|nested| find_stream(&nested, inherited_auth)),
        _ => None,
    }
}

fn object_string(values: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| values.get(*key)?.as_str())
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_owned)
}

fn object_u64(values: &serde_json::Map<String, Value>, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| {
            let value = values.get(*key)?;
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or_default()
}

fn first_non_empty(values: impl IntoIterator<Item = String>) -> String {
    values
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

fn to_song(track: Track) -> Option<Song> {
    if track.id.trim().is_empty() {
        return None;
    }
    let duration_ms = if track.duration > 1000 {
        track.duration as u64
    } else {
        (track.duration as u64).saturating_mul(1000)
    };
    let cover_url = track.album.url_cover.urls.first().map(|url| {
        if url.contains('~') {
            url.clone()
        } else {
            format!("{url}~c5_375x375.jpg")
        }
    });
    Some(Song {
        reference: SongRef {
            source: "soda".into(),
            scope: "song".into(),
            site_id: track.id,
        },
        title: track.name,
        artists: track
            .artists
            .into_iter()
            .filter(|artist| !artist.name.trim().is_empty())
            .map(|artist| Artist {
                id: None,
                name: artist.name,
            })
            .collect(),
        album: Some(AlbumRef {
            id: (!track.album.id.is_empty()).then_some(track.album.id),
            name: track.album.name,
            source: Some("soda".into()),
        }),
        duration_ms: (duration_ms > 0).then_some(duration_ms),
        cover_url,
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
    result_groups: Vec<ResultGroup>,
}

#[derive(Deserialize)]
struct ResultGroup {
    #[serde(default)]
    data: Vec<ResultItem>,
}

#[derive(Deserialize)]
struct ResultItem {
    entity: Entity,
}

#[derive(Deserialize)]
struct Entity {
    track: Track,
}

#[derive(Deserialize)]
struct Track {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    duration: u32,
    #[serde(default)]
    artists: Vec<TrackArtist>,
    #[serde(default)]
    album: TrackAlbum,
}

#[derive(Deserialize)]
struct TrackArtist {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct TrackAlbum {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url_cover: TrackImage,
}

#[derive(Default, Deserialize)]
struct TrackImage {
    #[serde(default)]
    urls: Vec<String>,
}

#[derive(Deserialize)]
struct TrackV2Response {
    #[serde(default)]
    status_code: i32,
    #[serde(default)]
    track_player: TrackPlayer,
}

#[derive(Default, Deserialize)]
struct TrackPlayer {
    #[serde(default)]
    url_player_info: Option<String>,
    #[serde(default)]
    video_model: Value,
}

#[derive(Deserialize)]
struct PlayerInfoResponse {
    #[serde(rename = "Result")]
    result: PlayerInfoResult,
}

#[derive(Deserialize)]
struct PlayerInfoResult {
    #[serde(rename = "Data")]
    data: PlayerInfoData,
}

#[derive(Deserialize)]
struct PlayerInfoData {
    #[serde(rename = "PlayInfoList", default)]
    play_info_list: Vec<PlayerInfoItem>,
}

#[derive(Deserialize)]
struct PlayerInfoItem {
    #[serde(rename = "MainPlayUrl", default)]
    main_url: String,
    #[serde(rename = "BackupPlayUrl", default)]
    backup_url: String,
    #[serde(rename = "PlayAuth", default)]
    play_auth: String,
    #[serde(rename = "Quality", default)]
    quality: String,
    #[serde(rename = "Format", default)]
    format: String,
    #[serde(rename = "Bitrate", default)]
    bitrate: u64,
    #[serde(rename = "Size", default)]
    size: u64,
    #[serde(rename = "Duration", default)]
    duration: f64,
}

struct Discovery {
    stream: Option<Stream>,
    player_info_url: Option<String>,
}

struct Stream {
    url: String,
    play_auth: String,
    quality: String,
    bitrate: u64,
    size: u64,
    duration: u64,
}

#[cfg(test)]
mod tests {
    use super::{parse_player_info, parse_search, parse_track_v2, SodaSource};
    use crate::domain::source::{MusicSource, SearchQuery};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    #[test]
    fn parses_search_fixture_and_skips_empty_track() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/soda/search.json"),
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].reference.site_id, "7300000000000000001");
        assert_eq!(result.items[0].duration_ms, Some(201_000));
    }

    #[test]
    fn discovers_encrypted_video_model_stream() {
        let result = parse_track_v2(include_bytes!(
            "../../../../tests/fixtures/soda/playback.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        let stream = result.stream.unwrap_or_else(|| unreachable!());
        assert_eq!(stream.url, "https://example.test/encrypted.m4a");
        assert_eq!(stream.play_auth, "auth-token");
        assert_eq!(stream.quality, "lossless");
    }

    #[test]
    fn selects_best_clear_player_info_stream() {
        let result = parse_player_info(include_bytes!(
            "../../../../tests/fixtures/soda/player-info.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.url, "https://example.test/320.m4a");
        assert!(result.play_auth.is_empty());
        assert_eq!(result.quality, "higher");
    }

    #[tokio::test]
    #[ignore = "requires live Soda service"]
    async fn resolves_live_resource() {
        let source = SodaSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .resolve_song(crate::domain::source::SongRef {
                source: "soda".into(),
                scope: "song".into(),
                site_id: "7304719759323564095".into(),
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    #[ignore = "requires live Soda service"]
    async fn searches_live_service() {
        let source = SodaSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 5,
                sources: vec!["soda".into()],
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }
}
