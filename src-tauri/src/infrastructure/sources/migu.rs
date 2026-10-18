use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::media::PlayableResource;
use crate::domain::source::{
    AlbumRef, Artist, MusicSource, SearchQuery, SearchResult, Song, SongRef, SourceCapabilities,
    SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::HttpClient;

const SEARCH_API: &str = "http://pd.musicapp.migu.cn/MIGUM2.0/v1.0/content/search_all.do";
const DETAIL_API: &str = "http://c.musicapp.migu.cn/MIGUM2.0/v1.0/content/resourceinfo.do";
const PLAYBACK_API: &str = "http://app.pd.nf.migu.cn/MIGUM2.0/v1.0/content/sub/listenSong.do";
const MAGIC_USER_ID: &str = "15548614588710179085069";
const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 9_1 like Mac OS X) AppleWebKit/601.1.46 Mobile/13B143 Safari/601.1";

pub struct MiguSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> MiguSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for MiguSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "migu"
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
            qualities: vec!["PQ".into(), "HQ".into(), "SQ".into()],
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let mut url = reqwest::Url::parse(SEARCH_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("ua", "Android_migu")
            .append_pair("version", "5.0.1")
            .append_pair("text", &query.query)
            .append_pair("pageNo", &query.page.to_string())
            .append_pair("pageSize", &query.page_size.to_string())
            .append_pair(
                "searchSwitch",
                r#"{"song":1,"album":0,"singer":0,"tagSong":0,"mvSong":0,"songlist":0,"bestShow":1}"#,
            );
        let response = self
            .client
            .get_with_headers(
                url.as_str(),
                &[
                    ("User-Agent", USER_AGENT.into()),
                    ("Referer", "http://music.migu.cn/".into()),
                    ("Cookie", self.cookie.value()),
                ],
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("migu returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        parse_search(&response.body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        if song.source != self.id() || song.scope != "song" || song.site_id.trim().is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid Migu song reference".into(),
                false,
            ));
        }
        let mut detail_url = reqwest::Url::parse(DETAIL_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        detail_url
            .query_pairs_mut()
            .append_pair("resourceType", "2")
            .append_pair("resourceId", &song.site_id);
        let response = self
            .client
            .get_with_headers(
                detail_url.as_str(),
                &[
                    ("User-Agent", USER_AGENT.into()),
                    ("Referer", "http://music.migu.cn/".into()),
                    ("Cookie", self.cookie.value()),
                ],
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("migu returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        let (resource_type, format_type) = parse_playback_format(&response.body)?;
        let mut playback_url = reqwest::Url::parse(PLAYBACK_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        playback_url
            .query_pairs_mut()
            .append_pair("toneFlag", &format_type)
            .append_pair("netType", "00")
            .append_pair("userId", MAGIC_USER_ID)
            .append_pair("ua", "Android_migu")
            .append_pair("version", "5.1")
            .append_pair("copyrightId", "0")
            .append_pair("contentId", &song.site_id)
            .append_pair("resourceType", &resource_type)
            .append_pair("channel", "0");
        Ok(PlayableResource {
            song,
            url: playback_url.into(),
            quality: format_type,
            duration_ms: None,
            expires_at: None,
        })
    }
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    let payload: SearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let songs = payload.song_result_data;
    let items = songs
        .result
        .into_iter()
        .filter(has_free_format)
        .filter_map(to_song)
        .collect();
    Ok(SearchResult {
        items,
        total: songs.total_count,
        page,
        page_size,
    })
}

fn has_free_format(song: &SearchSong) -> bool {
    song.rate_formats
        .iter()
        .chain(song.audio_formats.iter())
        .any(|format| {
            let vip = format
                .show_tag
                .iter()
                .chain(format.show_tags.iter())
                .any(|tag| tag == "vip");
            let paid = song.charge_auditions == "1"
                && format.price.parse::<u32>().unwrap_or_default() >= 200;
            !vip && !paid
        })
}

fn to_song(song: SearchSong) -> Option<Song> {
    let site_id = first_non_empty([&song.content_id, &song.copyright_id, &song.id])?;
    let artists = song
        .singers
        .iter()
        .chain(song.singer_list.iter())
        .chain(song.artists.iter())
        .filter(|artist| !artist.name.trim().is_empty())
        .map(|artist| Artist {
            id: (!artist.id.is_empty()).then(|| artist.id.clone()),
            name: artist.name.clone(),
        })
        .collect();
    let album = song.albums.first().map(|album| AlbumRef {
        id: (!album.id.is_empty()).then(|| album.id.clone()),
        name: album.name.clone(),
        source: Some("migu".into()),
    });
    let cover_url = song
        .img_items
        .iter()
        .find(|image| image.img_size_type == "02" && !image.img.is_empty())
        .or_else(|| song.img_items.iter().find(|image| !image.img.is_empty()))
        .map(|image| normalize_image(&image.img));
    Some(Song {
        reference: SongRef {
            source: "migu".into(),
            scope: "song".into(),
            site_id: site_id.to_owned(),
        },
        title: first_non_empty([&song.name, &song.song_name])
            .unwrap_or_default()
            .to_owned(),
        artists,
        album,
        duration_ms: (song.duration > 0).then(|| (song.duration as u64).saturating_mul(1000)),
        cover_url,
    })
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = &'a String>) -> Option<&'a str> {
    values
        .into_iter()
        .map(String::as_str)
        .find(|value| !value.trim().is_empty())
}

fn normalize_image(image: &str) -> String {
    if image.starts_with('/') {
        format!("https://d.musicapp.migu.cn{image}")
    } else {
        image.to_owned()
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
    #[serde(rename = "songResultData")]
    song_result_data: SearchPage,
}

#[derive(Deserialize)]
struct SearchPage {
    #[serde(default)]
    result: Vec<SearchSong>,
    #[serde(
        rename = "totalCount",
        default,
        deserialize_with = "deserialize_optional_u64"
    )]
    total_count: Option<u64>,
}

#[derive(Deserialize)]
struct SearchSong {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "songName", default)]
    song_name: String,
    #[serde(rename = "contentId", default)]
    content_id: String,
    #[serde(rename = "copyrightId", default)]
    copyright_id: String,
    #[serde(rename = "chargeAuditions", default)]
    charge_auditions: String,
    #[serde(default)]
    singers: Vec<ArtistItem>,
    #[serde(default)]
    artists: Vec<ArtistItem>,
    #[serde(rename = "singerList", default)]
    singer_list: Vec<ArtistItem>,
    #[serde(default)]
    albums: Vec<AlbumItem>,
    #[serde(rename = "imgItems", default)]
    img_items: Vec<ImageItem>,
    #[serde(rename = "rateFormats", default)]
    rate_formats: Vec<RateFormat>,
    #[serde(rename = "audioFormats", default)]
    audio_formats: Vec<RateFormat>,
    #[serde(default)]
    duration: u32,
}

#[derive(Deserialize)]
struct ArtistItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct AlbumItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct ImageItem {
    #[serde(rename = "imgSizeType", default)]
    img_size_type: String,
    #[serde(default)]
    img: String,
}

#[derive(Deserialize)]
struct RateFormat {
    #[serde(rename = "formatType", default)]
    format_type: String,
    #[serde(rename = "resourceType", default)]
    resource_type: String,
    #[serde(default)]
    size: String,
    #[serde(rename = "androidSize", default)]
    android_size: String,
    #[serde(default)]
    price: String,
    #[serde(rename = "showTag", default)]
    show_tag: Vec<String>,
    #[serde(rename = "showTags", default)]
    show_tags: Vec<String>,
}

#[derive(Deserialize)]
struct DetailResponse {
    #[serde(default)]
    resource: Vec<SearchSong>,
}

fn parse_playback_format(body: &[u8]) -> Result<(String, String), SourceError> {
    let payload: DetailResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let song = payload.resource.first().ok_or_else(|| {
        source_error(
            "SOURCE_PLAYBACK_UNAVAILABLE",
            "Migu song detail was not found".into(),
            false,
        )
    })?;
    song.rate_formats
        .iter()
        .chain(song.audio_formats.iter())
        .filter(|format| {
            let vip = format
                .show_tag
                .iter()
                .chain(format.show_tags.iter())
                .any(|tag| tag == "vip");
            let paid = song.charge_auditions == "1"
                && format.price.parse::<u32>().unwrap_or_default() >= 200;
            !vip && !paid && !format.format_type.is_empty() && !format.resource_type.is_empty()
        })
        .max_by_key(|format| {
            format
                .android_size
                .parse::<u64>()
                .ok()
                .or_else(|| format.size.parse().ok())
                .unwrap_or_default()
        })
        .map(|format| (format.resource_type.clone(), format.format_type.clone()))
        .ok_or_else(|| {
            source_error(
                "SOURCE_PLAYBACK_UNAVAILABLE",
                "Migu track has no free playable format".into(),
                false,
            )
        })
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
    use super::{parse_playback_format, parse_search, MiguSource, PLAYBACK_API};
    use crate::domain::source::{MusicSource, SearchQuery};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    #[test]
    fn parses_fixture_and_filters_paid_song() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/migu/search.json"),
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.total, Some(2));
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].reference.site_id, "CONTENT123");
        assert_eq!(
            result.items[0].cover_url.as_deref(),
            Some("https://d.musicapp.migu.cn/cover.jpg")
        );
    }

    #[test]
    fn selects_highest_free_playback_format() {
        let format = parse_playback_format(include_bytes!(
            "../../../../tests/fixtures/migu/playback.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(format, ("2".into(), "HQ".into()));
    }

    #[tokio::test]
    #[ignore = "requires live Migu service"]
    async fn resolves_live_resource() {
        let source = MiguSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let search = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 5,
                sources: vec!["migu".into()],
            })
            .await
            .unwrap_or_else(|_| unreachable!());
        let result = source.resolve_song(search.items[0].reference.clone()).await;
        assert!(result.is_ok(), "{result:?}");
        assert!(result
            .unwrap_or_else(|_| unreachable!())
            .url
            .starts_with(PLAYBACK_API));
    }

    #[tokio::test]
    #[ignore = "requires live Migu service"]
    async fn searches_live_service() {
        let source = MiguSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 5,
                sources: vec!["migu".into()],
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }
}
