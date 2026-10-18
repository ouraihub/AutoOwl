use async_trait::async_trait;
use base64::prelude::{Engine as _, BASE64_STANDARD};
use serde::Deserialize;

use crate::domain::collection::{Collection, CollectionType};
use crate::domain::lyrics::parse_lrc;
use crate::domain::media::{Lyrics, PlayableResource};
use crate::domain::source::{
    AlbumRef, Artist, CollectionDetail, CollectionPage, MusicSource, SearchQuery, SearchResult,
    Song, SongRef, SourceCapabilities, SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::{source_headers, HttpClient};

const SEARCH_API: &str = "https://cache.api.joox.com/openjoox/v3/search";
const PLAYLIST_DETAIL_API: &str = "https://cache.api.joox.com/openjoox/v3/playlist_detail";
const ALBUM_DETAIL_API: &str = "https://cache.api.joox.com/openjoox/v3/album_detail";
const PLAYBACK_API: &str = "https://api.joox.com/web-fcgi-bin/web_get_songinfo";
const LYRIC_API: &str = "https://api.joox.com/web-fcgi-bin/web_lyric";
/// JOOX is region locked and its web links are all `/hk/` (see
/// `domain::link::original_link`), so every request declares the same region.
const REGION: &str = "hk";
/// Search result type discriminators of the openjoox v3 endpoint.
const SEARCH_TYPE_SONG: &str = "0";
const SEARCH_TYPE_ALBUM: &str = "1";
const SEARCH_TYPE_PLAYLIST: &str = "2";

pub struct JooxSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> JooxSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for JooxSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "joox"
    }

    /// JOOX exposes cover art on every song payload and serves LRC through a
    /// dedicated endpoint. It has no QR login; a pasted cookie is what unlocks
    /// higher qualities for a subscriber.
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            song_search: true,
            playlist_search: true,
            album_search: true,
            lyrics: true,
            cover: true,
            cookie: true,
            qr_login: false,
            qualities: vec![
                "320kbps".into(),
                "192kbps".into(),
                "128kbps".into(),
                "96kbps".into(),
            ],
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let response = self.search(&query, SEARCH_TYPE_SONG).await?;
        parse_search(&response.body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        if song.source != self.id() || song.scope != "song" || song.site_id.trim().is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid JOOX song reference".into(),
                false,
            ));
        }
        let mut url = reqwest::Url::parse(PLAYBACK_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("songid", &song.site_id)
            .append_pair("lang", "zh_cn")
            .append_pair("country", REGION);
        let response = self.request(url.as_str()).await?;
        let (url, quality, duration_ms) = parse_playback(&response.body)?;
        Ok(PlayableResource {
            song,
            url,
            quality,
            duration_ms,
            expires_at: None,
        })
    }

    async fn lyrics(&self, song: SongRef) -> Result<Lyrics, SourceError> {
        if song.source != self.id() || song.site_id.trim().is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid JOOX song reference".into(),
                false,
            ));
        }
        let mut url = reqwest::Url::parse(LYRIC_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("musicid", &song.site_id)
            .append_pair("lang", "zh_cn")
            .append_pair("country", REGION);
        let response = self.request(url.as_str()).await?;
        parse_lyrics(&response.body)
    }

    async fn search_playlists(&self, query: SearchQuery) -> Result<CollectionPage, SourceError> {
        let response = self.search(&query, SEARCH_TYPE_PLAYLIST).await?;
        parse_collection_search(
            &response.body,
            CollectionType::Playlist,
            query.page,
            query.page_size,
        )
    }

    async fn search_albums(&self, query: SearchQuery) -> Result<CollectionPage, SourceError> {
        let response = self.search(&query, SEARCH_TYPE_ALBUM).await?;
        parse_collection_search(
            &response.body,
            CollectionType::Album,
            query.page,
            query.page_size,
        )
    }

    async fn playlist_detail(&self, id: &str) -> Result<CollectionDetail, SourceError> {
        let response = self
            .collection_detail(PLAYLIST_DETAIL_API, "playlist_id", id)
            .await?;
        parse_collection_detail(&response.body, id, CollectionType::Playlist)
    }

    async fn album_detail(&self, id: &str) -> Result<CollectionDetail, SourceError> {
        let response = self
            .collection_detail(ALBUM_DETAIL_API, "album_id", id)
            .await?;
        parse_collection_detail(&response.body, id, CollectionType::Album)
    }
}

impl<C> JooxSource<C>
where
    C: HttpClient,
{
    /// JOOX geofences its API; the forwarded-for header plus the region params
    /// are what keep it answering outside the licensed region.
    async fn request(
        &self,
        url: &str,
    ) -> Result<crate::infrastructure::http::HttpResponse, SourceError> {
        let mut headers = source_headers("joox", &self.cookie.value());
        headers.push(("X-Forwarded-For", "1.1.1.1".into()));
        let response = self
            .client
            .get_with_headers(url, &headers)
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("joox returned HTTP {}", response.status),
                response.status == 429 || response.status >= 500,
            ));
        }
        Ok(response)
    }

    async fn search(
        &self,
        query: &SearchQuery,
        search_type: &str,
    ) -> Result<crate::infrastructure::http::HttpResponse, SourceError> {
        let mut url = reqwest::Url::parse(SEARCH_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("country", REGION)
            .append_pair("lang", "zh_cn")
            .append_pair("type", search_type)
            .append_pair("index", &query.page.to_string())
            .append_pair("num", &query.page_size.to_string())
            .append_pair("keyword", &query.query);
        self.request(url.as_str()).await
    }

    async fn collection_detail(
        &self,
        api: &str,
        id_param: &str,
        id: &str,
    ) -> Result<crate::infrastructure::http::HttpResponse, SourceError> {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "JOOX 歌单/专辑 id 不能为空".into(),
                false,
            ));
        }
        let mut url = reqwest::Url::parse(api)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("country", REGION)
            .append_pair("lang", "zh_cn")
            .append_pair(id_param, trimmed);
        self.request(url.as_str()).await
    }
}

fn parse_playback(body: &[u8]) -> Result<(String, String, Option<u64>), SourceError> {
    let text = std::str::from_utf8(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?
        .trim();
    let json = text
        .strip_prefix("MusicInfoCallback(")
        .and_then(|value| value.strip_suffix(')'))
        .unwrap_or(text);
    let payload: PlaybackResponse = serde_json::from_str(json)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let (url, quality) = [
        (payload.url_320.as_str(), "320kbps"),
        (payload.url_192.as_str(), "192kbps"),
        (payload.url_128.as_str(), "128kbps"),
        (payload.url_96.as_str(), "96kbps"),
    ]
    .into_iter()
    .find(|(url, _)| url.starts_with("https://") || url.starts_with("http://"))
    .ok_or_else(|| {
        source_error(
            "SOURCE_PLAYBACK_UNAVAILABLE",
            "JOOX track has no playable stream".into(),
            false,
        )
    })?;
    Ok((
        url.into(),
        quality.into(),
        (payload.duration > 0).then(|| (payload.duration as u64).saturating_mul(1000)),
    ))
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    let payload: SearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let items = payload
        .section_list
        .into_iter()
        .flat_map(|section| section.item_list)
        .flat_map(|item| item.songs)
        .filter_map(|item| to_song(item.song_info))
        .collect();
    Ok(SearchResult {
        items,
        total: None,
        page,
        page_size,
    })
}

/// JOOX returns LRC as base64 inside a JSON envelope, and an empty `lyric`
/// means "no lyrics for this track" rather than a transport failure.
fn parse_lyrics(body: &[u8]) -> Result<Lyrics, SourceError> {
    let payload: LyricResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let original = decode_lyric_field(&payload.lyric)?;
    let translated = decode_lyric_field(&payload.translation)?;
    let lines = original.as_deref().map(parse_lrc).unwrap_or_default();
    Ok(Lyrics {
        original,
        translated,
        romanized: None,
        lines,
    })
}

/// Tolerates plain text as well as base64: JOOX has shipped both.
fn decode_lyric_field(value: &str) -> Result<Option<String>, SourceError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match BASE64_STANDARD.decode(trimmed) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) if !text.trim().is_empty() => Ok(Some(text)),
            Ok(_) => Ok(None),
            Err(error) => Err(source_error(
                "SOURCE_RESPONSE_INVALID",
                error.to_string(),
                false,
            )),
        },
        Err(_) => Ok(Some(trimmed.to_owned())),
    }
}

fn parse_collection_search(
    body: &[u8],
    content_type: CollectionType,
    page: u32,
    page_size: u32,
) -> Result<CollectionPage, SourceError> {
    let payload: CollectionSearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let items = payload
        .section_list
        .into_iter()
        .flat_map(|section| section.item_list)
        .filter_map(|item| item.into_collection(content_type))
        .collect();
    Ok(CollectionPage {
        items,
        total: None,
        page,
        page_size,
    })
}

fn parse_collection_detail(
    body: &[u8],
    id: &str,
    content_type: CollectionType,
) -> Result<CollectionDetail, SourceError> {
    let payload: CollectionDetailResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let songs: Vec<Song> = payload.tracks.into_iter().filter_map(to_song).collect();
    Ok(CollectionDetail {
        collection: Collection {
            id: id.to_owned(),
            source: "joox".into(),
            content_type,
            name: payload.name,
            description: payload.description.filter(|value| !value.trim().is_empty()),
            creator: payload.creator.filter(|value| !value.trim().is_empty()),
            cover_url: payload.cover.filter(|value| !value.trim().is_empty()),
            track_count: Some(songs.len() as u64),
        },
        songs,
    })
}

fn to_song(info: SongInfo) -> Option<Song> {
    if info.id.trim().is_empty() {
        return None;
    }
    let cover_url = info
        .images
        .iter()
        .find(|image| image.width == 300)
        .or_else(|| info.images.first())
        .map(|image| image.url.clone());
    Some(Song {
        reference: SongRef {
            source: "joox".into(),
            scope: "song".into(),
            site_id: info.id,
        },
        title: info.name,
        artists: info
            .artist_list
            .into_iter()
            .map(|artist| Artist {
                id: None,
                name: artist.name,
            })
            .collect(),
        album: Some(AlbumRef {
            id: None,
            name: info.album_name,
            source: Some("joox".into()),
        }),
        duration_ms: Some((info.play_duration as u64).saturating_mul(1000)),
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
    section_list: Vec<SearchSection>,
}

#[derive(Deserialize)]
struct SearchSection {
    #[serde(default)]
    item_list: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    #[serde(rename = "song", default)]
    songs: Vec<SongItem>,
}

#[derive(Deserialize)]
struct SongItem {
    song_info: SongInfo,
}

#[derive(Deserialize)]
struct SongInfo {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    album_name: String,
    #[serde(default)]
    artist_list: Vec<SongArtist>,
    #[serde(default)]
    play_duration: u32,
    #[serde(default)]
    images: Vec<SongImage>,
}

#[derive(Deserialize)]
struct SongArtist {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct SongImage {
    #[serde(default)]
    width: u32,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
struct PlaybackResponse {
    #[serde(rename = "r320Url", default)]
    url_320: String,
    #[serde(rename = "r192Url", default)]
    url_192: String,
    #[serde(rename = "mp3Url", default)]
    url_128: String,
    #[serde(rename = "m4aUrl", default)]
    url_96: String,
    #[serde(rename = "minterval", default)]
    duration: u32,
}

#[derive(Deserialize)]
struct LyricResponse {
    #[serde(default)]
    lyric: String,
    #[serde(default)]
    translation: String,
}

#[derive(Deserialize)]
struct CollectionSearchResponse {
    #[serde(default)]
    section_list: Vec<CollectionSearchSection>,
}

#[derive(Deserialize)]
struct CollectionSearchSection {
    #[serde(default)]
    item_list: Vec<CollectionSearchItem>,
}

/// JOOX names the same concepts differently per surface: playlist hits carry
/// `playlist_id`/`name`, album hits carry `album_id`/`album_name`. Aliases keep
/// one struct for both instead of duplicating the parse.
#[derive(Deserialize)]
struct CollectionSearchItem {
    #[serde(default, alias = "playlist_id", alias = "album_id")]
    id: String,
    #[serde(default, alias = "album_name", alias = "playlist_name")]
    name: String,
    #[serde(default, alias = "singer_name", alias = "creator_nick")]
    creator: Option<String>,
    #[serde(default, alias = "pic", alias = "album_pic", alias = "playlist_pic")]
    cover: Option<String>,
    #[serde(default, alias = "song_num", alias = "track_num")]
    track_count: Option<u64>,
}

impl CollectionSearchItem {
    fn into_collection(self, content_type: CollectionType) -> Option<Collection> {
        if self.id.trim().is_empty() {
            return None;
        }
        Some(Collection {
            id: self.id,
            source: "joox".into(),
            content_type,
            name: self.name,
            description: None,
            creator: self.creator.filter(|value| !value.trim().is_empty()),
            cover_url: self.cover.filter(|value| !value.trim().is_empty()),
            track_count: self.track_count,
        })
    }
}

#[derive(Deserialize)]
struct CollectionDetailResponse {
    #[serde(default, alias = "album_name", alias = "playlist_name")]
    name: String,
    #[serde(default, alias = "desc")]
    description: Option<String>,
    #[serde(default, alias = "singer_name", alias = "creator_nick")]
    creator: Option<String>,
    #[serde(default, alias = "pic", alias = "album_pic")]
    cover: Option<String>,
    #[serde(default, alias = "tracks", alias = "song_list", alias = "itemlist")]
    tracks: Vec<SongInfo>,
}

#[cfg(test)]
mod tests {
    use super::{parse_playback, parse_search, JooxSource};
    use crate::domain::source::{MusicSource, SearchQuery};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    #[test]
    fn parses_fixture_and_prefers_300_pixel_cover() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/joox/search.json"),
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].reference.site_id, "JOOX123");
        assert_eq!(
            result.items[0].cover_url.as_deref(),
            Some("https://example.test/300.jpg")
        );
    }

    #[test]
    fn parses_callback_playback_fixture() {
        let resource = parse_playback(include_bytes!(
            "../../../../tests/fixtures/joox/playback.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(resource.0, "https://example.test/song-320.mp3");
        assert_eq!(resource.1, "320kbps");
        assert_eq!(resource.2, Some(186_000));
    }

    #[tokio::test]
    #[ignore = "requires live JOOX service"]
    async fn resolves_live_resource() {
        let source = JooxSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let search = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 5,
                sources: vec!["joox".into()],
            })
            .await
            .unwrap_or_else(|_| unreachable!());
        let result = source.resolve_song(search.items[0].reference.clone()).await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    #[ignore = "requires live JOOX service"]
    async fn searches_live_service() {
        let source = JooxSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 20,
                sources: vec!["joox".into()],
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }
}
