use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::collection::{Collection, CollectionType};
use crate::domain::media::PlayableResource;
use crate::domain::source::{
    AlbumRef, Artist, CollectionDetail, CollectionPage, MusicSource, SearchQuery, SearchResult,
    Song, SongRef, SourceCapabilities, SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::{source_headers, HttpClient};

const SEARCH_API: &str = "https://api.bilibili.com/x/web-interface/search/type";
/// The unprefixed `x/web-interface/view` now answers **HTTP 412** to every
/// caller regardless of cookie or headers -- verified live against four bvids.
/// The `wbi/` route returns the same payload and needs no signature params.
const VIEW_API: &str = "https://api.bilibili.com/x/web-interface/wbi/view";
const PLAYBACK_API: &str = "https://api.bilibili.com/x/player/playurl";
const SEASONS_SERIES_API: &str = "https://api.bilibili.com/x/polymer/web-space/seasons_series_list";
const SEASON_ARCHIVES_API: &str =
    "https://api.bilibili.com/x/polymer/web-space/seasons_archives_list";
const SERIES_ARCHIVES_API: &str = "https://api.bilibili.com/x/series/archives";
/// Placeholder `buvid3` cookie; see [`with_buvid`].
const BUVID_PLACEHOLDER: &str = "buvid3=00000000-0000-0000-0000-00000000000000000infoc";
/// Collection listings resolve each archive through the view API, so the page
/// size doubles as a cap on fan-out requests.
const ARCHIVE_PAGE_SIZE: u32 = 30;

pub struct BilibiliSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> BilibiliSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for BilibiliSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "bilibili"
    }

    /// Bilibili hosts videos, not tagged tracks: there is no lyrics endpoint
    /// (subtitles are per-video CC data, not song lyrics), and no album concept
    /// beyond a multi-part video, so `album_search` stays off while
    /// `album_detail` exposes a video's parts.
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            song_search: true,
            playlist_search: true,
            album_search: false,
            lyrics: false,
            cover: true,
            cookie: true,
            qr_login: true,
            qualities: vec![
                "hires".into(),
                "dolby".into(),
                "192kbps".into(),
                "132kbps".into(),
                "64kbps".into(),
            ],
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let mut url = reqwest::Url::parse(SEARCH_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("search_type", "video")
            .append_pair("keyword", &query.query)
            .append_pair("page", &query.page.to_string())
            .append_pair("page_size", &query.page_size.to_string());
        let response = self.request(url.as_str()).await?;
        let payload = parse_search(&response.body)?;
        let mut items = Vec::with_capacity(payload.data.result.len());
        for item in payload.data.result {
            // One view request per hit is unavoidable: search results carry no
            // cid, which playback requires. A failed hit is skipped rather than
            // failing the whole page.
            let Ok(view) = self.video_view(&item.bvid).await else {
                continue;
            };
            if let Some(song) = to_song(item, view) {
                items.push(song);
            }
        }
        Ok(SearchResult {
            items,
            total: payload.data.total,
            page: query.page,
            page_size: query.page_size,
        })
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        let Some((bvid, cid)) = song.site_id.split_once('|') else {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid Bilibili song reference".into(),
                false,
            ));
        };
        if song.source != self.id()
            || song.scope != "song"
            || bvid.trim().is_empty()
            || cid.parse::<u64>().is_err()
        {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid Bilibili song reference".into(),
                false,
            ));
        }
        let mut url = reqwest::Url::parse(PLAYBACK_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("fnval", "80")
            .append_pair("qn", "127")
            .append_pair("bvid", bvid)
            .append_pair("cid", cid);
        let response = self.request(url.as_str()).await?;
        let (url, quality) = parse_playback(&response.body)?;
        Ok(PlayableResource {
            song,
            url,
            quality,
            duration_ms: None,
            expires_at: None,
        })
    }

    /// Bilibili has no editorial playlists; the closest equivalent is a
    /// creator's collections (`season`) and series, so the query is read as a
    /// user id or a space/video URL containing one.
    async fn search_playlists(&self, query: SearchQuery) -> Result<CollectionPage, SourceError> {
        let mid = extract_mid(&query.query).ok_or_else(|| {
            source_error(
                "INVALID_ARGUMENT",
                "Bilibili 歌单检索需要 UP 主 UID 或空间链接".into(),
                false,
            )
        })?;
        let mut url = reqwest::Url::parse(SEASONS_SERIES_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("mid", &mid)
            .append_pair("page_num", &query.page.to_string())
            .append_pair("page_size", &query.page_size.to_string());
        let response = self.request(url.as_str()).await?;
        parse_collections(&response.body, &mid, query.page, query.page_size)
    }

    /// Lists a collection's videos, one track per video.
    async fn playlist_detail(&self, id: &str) -> Result<CollectionDetail, SourceError> {
        let reference = CollectionRef::parse(id)?;
        let mut url = reqwest::Url::parse(reference.archives_api())
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("mid", &reference.mid);
            pairs.append_pair("page_num", "1");
            pairs.append_pair("page_size", &ARCHIVE_PAGE_SIZE.to_string());
            match reference.kind {
                CollectionKind::Season => {
                    pairs.append_pair("season_id", &reference.list_id);
                }
                CollectionKind::Series => {
                    pairs.append_pair("series_id", &reference.list_id);
                    // The series endpoint paginates with pn/ps instead.
                    pairs.append_pair("pn", "1");
                    pairs.append_pair("ps", &ARCHIVE_PAGE_SIZE.to_string());
                }
            }
        }
        let response = self.request(url.as_str()).await?;
        let archives = parse_archives(&response.body)?;
        let mut songs = Vec::with_capacity(archives.items.len());
        for archive in &archives.items {
            // A collection entry is one video; take its first part so a
            // collection maps one-to-one onto tracks.
            songs.extend(self.video_songs(&archive.bvid).await?.into_iter().take(1));
        }
        Ok(CollectionDetail {
            collection: Collection {
                id: id.to_owned(),
                source: "bilibili".into(),
                content_type: CollectionType::Playlist,
                name: archives.name.unwrap_or_else(|| id.to_owned()),
                description: None,
                creator: None,
                cover_url: archives.cover,
                track_count: archives.total.or(Some(archives.items.len() as u64)),
            },
            songs,
        })
    }

    /// A multi-part video is Bilibili's only album-shaped object: every part
    /// becomes a track of the video it belongs to.
    async fn album_detail(&self, id: &str) -> Result<CollectionDetail, SourceError> {
        let bvid = id.trim();
        if bvid.is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "Bilibili 专辑需要 bvid".into(),
                false,
            ));
        }
        let view = self.video_view(bvid).await?;
        let songs = songs_from_view(bvid, &view);
        if songs.is_empty() {
            return Err(source_error(
                "SOURCE_TRACK_NOT_FOUND",
                "Bilibili 视频没有可用分P".into(),
                false,
            ));
        }
        Ok(CollectionDetail {
            collection: Collection {
                id: bvid.to_owned(),
                source: "bilibili".into(),
                content_type: CollectionType::Album,
                name: clean_title(&view.title),
                description: (!view.desc.trim().is_empty()).then(|| view.desc.clone()),
                creator: (!view.owner.name.is_empty()).then(|| view.owner.name.clone()),
                cover_url: (!view.pic.is_empty()).then(|| normalize_cover(&view.pic)),
                track_count: Some(songs.len() as u64),
            },
            songs,
        })
    }
}

impl<C> BilibiliSource<C>
where
    C: HttpClient,
{
    async fn request(
        &self,
        url: &str,
    ) -> Result<crate::infrastructure::http::HttpResponse, SourceError> {
        let cookie = self.cookie.value();
        let mut headers = source_headers("bilibili", &with_buvid(&cookie));
        headers.push(("Origin", "https://www.bilibili.com".to_owned()));
        let response = self
            .client
            .get_with_headers(url, &headers)
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("bilibili returned HTTP {}", response.status),
                response.status == 412 || response.status == 429 || response.status >= 500,
            ));
        }
        Ok(response)
    }

    async fn video_view(&self, bvid: &str) -> Result<ViewData, SourceError> {
        let mut url = reqwest::Url::parse(VIEW_API)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        url.query_pairs_mut().append_pair("bvid", bvid);
        let response = self.request(url.as_str()).await?;
        parse_view(&response.body)
    }

    /// Every part of a video, in order.
    async fn video_songs(&self, bvid: &str) -> Result<Vec<Song>, SourceError> {
        let view = self.video_view(bvid).await?;
        Ok(songs_from_view(bvid, &view))
    }
}

/// Ensures the outgoing cookie carries a `buvid3`, bilibili's anti-crawl gate.
///
/// Without it `api.bilibili.com` answers **HTTP 412** to every request; a
/// browser picks the value up from a `Set-Cookie` on `www.bilibili.com`. The
/// gate only checks for presence -- verified live, `buvid3=x` and a well-formed
/// UUID both return 20 search rows -- so a fixed placeholder avoids spending a
/// homepage round trip per call.
fn with_buvid(cookie: &str) -> String {
    let trimmed = cookie.trim();
    if trimmed.contains("buvid3=") {
        return trimmed.to_owned();
    }
    if trimmed.is_empty() {
        return BUVID_PLACEHOLDER.to_owned();
    }
    format!("{}; {BUVID_PLACEHOLDER}", trimmed.trim_end_matches(';'))
}

/// Reads a UP 主 UID out of a bare number or a bilibili space/video URL.
fn extract_mid(query: &str) -> Option<String> {
    let trimmed = query.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|character| character.is_ascii_digit()) {
        return Some(trimmed.to_owned());
    }
    let tail = trimmed.split("space.bilibili.com/").nth(1)?;
    let digits: String = tail
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    (!digits.is_empty()).then_some(digits)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollectionKind {
    Season,
    Series,
}

/// A collection is addressed by three parts because both Bilibili archive
/// endpoints require the owning user id alongside the list id.
#[derive(Debug, Clone)]
struct CollectionRef {
    kind: CollectionKind,
    mid: String,
    list_id: String,
}

impl CollectionRef {
    fn parse(id: &str) -> Result<Self, SourceError> {
        let mut parts = id.trim().splitn(3, '/');
        let kind = match parts.next() {
            Some("season") => CollectionKind::Season,
            Some("series") => CollectionKind::Series,
            _ => {
                return Err(source_error(
                    "INVALID_ARGUMENT",
                    "Bilibili 合集 id 需形如 season/{mid}/{id}".into(),
                    false,
                ))
            }
        };
        let mid = parts.next().unwrap_or_default().trim().to_owned();
        let list_id = parts.next().unwrap_or_default().trim().to_owned();
        if mid.is_empty() || list_id.is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "Bilibili 合集 id 需形如 season/{mid}/{id}".into(),
                false,
            ));
        }
        Ok(Self { kind, mid, list_id })
    }

    fn archives_api(&self) -> &'static str {
        match self.kind {
            CollectionKind::Season => SEASON_ARCHIVES_API,
            CollectionKind::Series => SERIES_ARCHIVES_API,
        }
    }
}

fn parse_search(body: &[u8]) -> Result<SearchResponse, SourceError> {
    let payload: SearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    if payload.code != 0 {
        return Err(source_error(
            "SOURCE_API_ERROR",
            format!("bilibili returned code {}", payload.code),
            payload.code == -412,
        ));
    }
    Ok(payload)
}

fn parse_view(body: &[u8]) -> Result<ViewData, SourceError> {
    let payload: ViewResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    if payload.code != 0 {
        return Err(source_error(
            "SOURCE_API_ERROR",
            format!("bilibili returned code {}", payload.code),
            payload.code == -412,
        ));
    }
    Ok(payload.data)
}

/// Both `seasons` and `series` entries of a creator's space, flattened into one
/// page because callers treat them alike.
fn parse_collections(
    body: &[u8],
    mid: &str,
    page: u32,
    page_size: u32,
) -> Result<CollectionPage, SourceError> {
    let payload: SeasonsSeriesResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    if payload.code != 0 {
        return Err(source_error(
            "SOURCE_API_ERROR",
            format!("bilibili returned code {}", payload.code),
            payload.code == -412,
        ));
    }
    let items = payload
        .data
        .items_lists
        .into_iter()
        .flat_map(|lists| {
            let seasons = lists.seasons_list.into_iter().map(|entry| {
                to_collection(CollectionKind::Season, mid, entry.meta, entry.recent_aids)
            });
            let series = lists.series_list.into_iter().map(|entry| {
                to_collection(CollectionKind::Series, mid, entry.meta, entry.recent_aids)
            });
            seasons.chain(series)
        })
        .collect();
    Ok(CollectionPage {
        items,
        total: payload.data.items_lists_page.total,
        page,
        page_size,
    })
}

fn to_collection(
    kind: CollectionKind,
    mid: &str,
    meta: CollectionMeta,
    recent: Vec<i64>,
) -> Collection {
    let prefix = match kind {
        CollectionKind::Season => "season",
        CollectionKind::Series => "series",
    };
    Collection {
        id: format!("{prefix}/{mid}/{}", meta.id),
        source: "bilibili".into(),
        content_type: CollectionType::Playlist,
        name: meta.name,
        description: (!meta.description.trim().is_empty()).then_some(meta.description),
        creator: (!meta.mid_name.trim().is_empty()).then_some(meta.mid_name),
        cover_url: (!meta.cover.is_empty()).then(|| normalize_cover(&meta.cover)),
        track_count: meta.total.or(Some(recent.len() as u64)),
    }
}

/// The season and series archive endpoints differ only in where the list name
/// lives, so one parser covers both.
fn parse_archives(body: &[u8]) -> Result<ArchiveList, SourceError> {
    let payload: ArchivesResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    if payload.code != 0 {
        return Err(source_error(
            "SOURCE_API_ERROR",
            format!("bilibili returned code {}", payload.code),
            payload.code == -412,
        ));
    }
    let meta = payload.data.meta;
    Ok(ArchiveList {
        name: meta
            .as_ref()
            .map(|meta| meta.name.clone())
            .filter(|name| !name.trim().is_empty()),
        cover: meta
            .as_ref()
            .filter(|meta| !meta.cover.is_empty())
            .map(|meta| normalize_cover(&meta.cover)),
        total: payload.data.page.total,
        items: payload
            .data
            .archives
            .into_iter()
            .filter(|archive| !archive.bvid.trim().is_empty())
            .collect(),
    })
}

/// One track per part of a video.
fn songs_from_view(bvid: &str, view: &ViewData) -> Vec<Song> {
    let root_title = clean_title(&view.title);
    let cover = (!view.pic.is_empty()).then(|| normalize_cover(&view.pic));
    view.pages
        .iter()
        .map(|page| Song {
            reference: SongRef {
                source: "bilibili".into(),
                scope: "song".into(),
                site_id: format!("{bvid}|{}", page.cid),
            },
            title: part_title(&root_title, &page.part),
            artists: vec![Artist {
                id: (view.owner.mid > 0).then(|| view.owner.mid.to_string()),
                name: view.owner.name.clone(),
            }],
            album: Some(AlbumRef {
                id: Some(bvid.to_owned()),
                name: root_title.clone(),
                source: Some("bilibili".into()),
            }),
            duration_ms: Some((page.duration as u64).saturating_mul(1000)),
            cover_url: cover.clone(),
        })
        .collect()
}

/// A single-part video repeats its title in `part`; only distinct part names
/// are worth appending.
fn part_title(root_title: &str, part: &str) -> String {
    if part.trim().is_empty() || part == root_title {
        root_title.to_owned()
    } else {
        format!("{root_title} - {part}")
    }
}

#[derive(Debug)]
struct ArchiveList {
    name: Option<String>,
    cover: Option<String>,
    total: Option<u64>,
    items: Vec<Archive>,
}

fn parse_playback(body: &[u8]) -> Result<(String, String), SourceError> {
    let payload: PlaybackResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    if payload.code != 0 {
        return Err(source_error(
            "SOURCE_API_ERROR",
            format!("bilibili returned code {}", payload.code),
            payload.code == -412,
        ));
    }
    if let Some(audio) = payload
        .data
        .dash
        .audio
        .into_iter()
        .filter(|audio| is_http_url(&audio.base_url))
        .max_by_key(|audio| audio.id)
    {
        return Ok((audio.base_url, audio.id.to_string()));
    }
    payload
        .data
        .durl
        .into_iter()
        .find(|item| is_http_url(&item.url))
        .map(|item| (item.url, "durl".into()))
        .ok_or_else(|| {
            source_error(
                "SOURCE_PLAYBACK_UNAVAILABLE",
                "Bilibili response contained no playable audio URL".into(),
                false,
            )
        })
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

/// A search hit becomes its video's first part; the search payload supplies the
/// title and cover because they are already highlighted/normalised there.
fn to_song(search: SearchItem, view: ViewData) -> Option<Song> {
    let page = view.pages.first()?;
    let root_title = clean_title(&search.title);
    Some(Song {
        reference: SongRef {
            source: "bilibili".into(),
            scope: "song".into(),
            site_id: format!("{}|{}", search.bvid, page.cid),
        },
        title: part_title(&root_title, &page.part),
        artists: vec![Artist {
            id: (view.owner.mid > 0).then(|| view.owner.mid.to_string()),
            name: search.author,
        }],
        album: Some(AlbumRef {
            id: Some(search.bvid),
            name: root_title,
            source: Some("bilibili".into()),
        }),
        duration_ms: Some((page.duration as u64).saturating_mul(1000)),
        cover_url: (!search.pic.is_empty()).then(|| normalize_cover(&search.pic)),
    })
}

fn clean_title(value: &str) -> String {
    value
        .replace("<em class=\"keyword\">", "")
        .replace("</em>", "")
}

fn normalize_cover(value: &str) -> String {
    if value.starts_with("//") {
        format!("https:{value}")
    } else {
        value.to_owned()
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
    code: i32,
    data: SearchData,
}

#[derive(Deserialize)]
struct SearchData {
    #[serde(default)]
    result: Vec<SearchItem>,
    #[serde(rename = "numResults", default)]
    total: Option<u64>,
}

#[derive(Deserialize)]
struct SearchItem {
    bvid: String,
    title: String,
    author: String,
    #[serde(default)]
    pic: String,
}

#[derive(Deserialize)]
struct ViewResponse {
    code: i32,
    data: ViewData,
}

#[derive(Debug, Deserialize)]
struct ViewData {
    #[serde(default)]
    title: String,
    #[serde(default)]
    desc: String,
    #[serde(default)]
    pic: String,
    #[serde(default)]
    owner: ViewOwner,
    #[serde(default)]
    pages: Vec<ViewPage>,
}

#[derive(Debug, Default, Deserialize)]
struct ViewOwner {
    #[serde(default)]
    mid: i64,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct SeasonsSeriesResponse {
    code: i32,
    #[serde(default)]
    data: SeasonsSeriesData,
}

#[derive(Default, Deserialize)]
struct SeasonsSeriesData {
    #[serde(default)]
    items_lists: Vec<SeasonsSeriesLists>,
    #[serde(default)]
    items_lists_page: ArchivePage,
}

#[derive(Deserialize)]
struct SeasonsSeriesLists {
    #[serde(default)]
    seasons_list: Vec<CollectionEntry>,
    #[serde(default)]
    series_list: Vec<CollectionEntry>,
}

#[derive(Deserialize)]
struct CollectionEntry {
    meta: CollectionMeta,
    #[serde(default)]
    recent_aids: Vec<i64>,
}

#[derive(Deserialize)]
struct CollectionMeta {
    #[serde(alias = "season_id", alias = "series_id")]
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    cover: String,
    /// `season` payloads name this `total`; `series` payloads use `total`
    /// inside meta as well, so one field covers both.
    #[serde(default)]
    total: Option<u64>,
    #[serde(rename = "mid_name", default)]
    mid_name: String,
}

#[derive(Deserialize)]
struct ArchivesResponse {
    code: i32,
    #[serde(default)]
    data: ArchivesData,
}

#[derive(Default, Deserialize)]
struct ArchivesData {
    #[serde(default)]
    meta: Option<ArchivesMeta>,
    #[serde(default)]
    page: ArchivePage,
    #[serde(default)]
    archives: Vec<Archive>,
}

#[derive(Deserialize)]
struct ArchivesMeta {
    #[serde(default)]
    name: String,
    #[serde(default)]
    cover: String,
}

#[derive(Default, Deserialize)]
struct ArchivePage {
    #[serde(default)]
    total: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Archive {
    #[serde(default)]
    bvid: String,
}

#[derive(Debug, Deserialize)]
struct ViewPage {
    cid: i64,
    #[serde(default)]
    part: String,
    #[serde(default)]
    duration: u32,
}

#[derive(Deserialize)]
struct PlaybackResponse {
    code: i32,
    #[serde(default)]
    data: PlaybackData,
}

#[derive(Default, Deserialize)]
struct PlaybackData {
    #[serde(default)]
    dash: PlaybackDash,
    #[serde(default)]
    durl: Vec<PlaybackDurl>,
}

#[derive(Default, Deserialize)]
struct PlaybackDash {
    #[serde(default)]
    audio: Vec<PlaybackAudio>,
}

#[derive(Deserialize)]
struct PlaybackAudio {
    id: i32,
    #[serde(rename = "baseUrl")]
    base_url: String,
}

#[derive(Deserialize)]
struct PlaybackDurl {
    url: String,
}

#[cfg(test)]
mod tests {
    use super::{
        extract_mid, parse_archives, parse_collections, parse_playback, parse_search, parse_view,
        songs_from_view, to_song, with_buvid, BilibiliSource, CollectionRef, ViewResponse,
    };
    use crate::domain::collection::CollectionType;
    use crate::domain::source::{MusicSource, SearchQuery};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    fn multipart_view() -> super::ViewData {
        parse_view(include_bytes!(
            "../../../../tests/fixtures/bilibili/view-multipart.json"
        ))
        .unwrap_or_else(|_| unreachable!())
    }

    #[test]
    fn multi_part_video_yields_one_track_per_part() {
        let view = multipart_view();
        let songs = songs_from_view("BV1aa4y1x7A1", &view);
        assert_eq!(songs.len(), 3);
        assert_eq!(
            songs
                .iter()
                .map(|song| song.reference.site_id.as_str())
                .collect::<Vec<_>>(),
            [
                "BV1aa4y1x7A1|1001",
                "BV1aa4y1x7A1|1002",
                "BV1aa4y1x7A1|1003"
            ]
        );
        assert_eq!(songs[0].title, "现场演奏合辑 - 开场");
        // An unnamed part falls back to the video title alone.
        assert_eq!(songs[2].title, "现场演奏合辑");
        assert_eq!(songs[1].duration_ms, Some(245_000));
        assert_eq!(songs[0].artists[0].name, "示例 UP 主");
        assert_eq!(
            songs[0].cover_url.as_deref(),
            Some("https://example.test/cover.jpg")
        );
    }

    #[test]
    fn collection_page_flattens_seasons_and_series() {
        let page = parse_collections(
            include_bytes!("../../../../tests/fixtures/bilibili/seasons_series.json"),
            "24680",
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(page.total, Some(2));
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["season/24680/778899", "series/24680/445566"]
        );
        assert_eq!(page.items[0].name, "钢琴合集");
        assert_eq!(page.items[0].track_count, Some(3));
        assert!(matches!(
            page.items[0].content_type,
            CollectionType::Playlist
        ));
        assert_eq!(
            page.items[0].cover_url.as_deref(),
            Some("https://example.test/season.jpg")
        );
        // A series without a declared total falls back to its recent entries.
        assert_eq!(page.items[1].track_count, Some(2));
        assert_eq!(page.items[1].description, None);
    }

    #[test]
    fn archive_list_drops_entries_without_bvid() {
        let archives = parse_archives(include_bytes!(
            "../../../../tests/fixtures/bilibili/season_archives.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(archives.name.as_deref(), Some("钢琴合集"));
        assert_eq!(archives.total, Some(2));
        assert_eq!(
            archives
                .items
                .iter()
                .map(|archive| archive.bvid.as_str())
                .collect::<Vec<_>>(),
            ["BV1aa4y1x7A1", "BV1bb4y1x7A2"]
        );
    }

    #[test]
    fn empty_collection_payload_yields_empty_page() {
        let page = parse_collections(br#"{"code":0,"data":{}}"#, "24680", 2, 20)
            .unwrap_or_else(|_| unreachable!());
        assert!(page.items.is_empty());
        assert_eq!(page.total, None);
        assert_eq!(page.page, 2);
    }

    #[test]
    fn malformed_and_error_payloads_surface_source_errors() {
        let malformed = parse_collections(b"not json", "24680", 1, 20)
            .expect_err("malformed payload must fail");
        assert_eq!(malformed.code, "SOURCE_RESPONSE_INVALID");
        let rejected =
            parse_archives(br#"{"code":-412,"data":{}}"#).expect_err("api error code must fail");
        assert_eq!(rejected.code, "SOURCE_API_ERROR");
        // -412 is Bilibili's rate limit, so the caller may retry.
        assert!(rejected.retryable);
        assert!(
            !parse_view(br#"{"code":-404,"data":{}}"#)
                .expect_err("missing video must fail")
                .retryable
        );
    }

    #[test]
    fn collection_ids_require_kind_owner_and_list() {
        let reference =
            CollectionRef::parse("season/24680/778899").unwrap_or_else(|_| unreachable!());
        assert_eq!(reference.mid, "24680");
        assert_eq!(reference.list_id, "778899");
        for invalid in ["", "season", "season/24680", "album/24680/1", "/24680/1"] {
            assert_eq!(
                CollectionRef::parse(invalid)
                    .expect_err("invalid id must fail")
                    .code,
                "INVALID_ARGUMENT",
                "{invalid}"
            );
        }
    }

    #[test]
    fn buvid_is_injected_only_when_absent() {
        assert_eq!(with_buvid(""), super::BUVID_PLACEHOLDER);
        assert_eq!(
            with_buvid("  SESSDATA=abc;  "),
            format!("SESSDATA=abc; {}", super::BUVID_PLACEHOLDER)
        );
        let existing = "buvid3=real-value; SESSDATA=abc";
        assert_eq!(with_buvid(existing), existing);
    }

    #[test]
    fn mid_comes_from_digits_or_space_url() {
        assert_eq!(extract_mid(" 24680 ").as_deref(), Some("24680"));
        assert_eq!(
            extract_mid("https://space.bilibili.com/24680/channel/series").as_deref(),
            Some("24680")
        );
        assert_eq!(extract_mid("周杰伦"), None);
        assert_eq!(extract_mid("https://www.bilibili.com/video/BV1xx"), None);
    }

    #[test]
    fn maps_search_and_view_fixtures() {
        let mut search = parse_search(include_bytes!(
            "../../../../tests/fixtures/bilibili/search.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        let view: ViewResponse = serde_json::from_slice(include_bytes!(
            "../../../../tests/fixtures/bilibili/view.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        let song =
            to_song(search.data.result.remove(0), view.data).unwrap_or_else(|| unreachable!());
        assert_eq!(song.reference.site_id, "BV1fixture|123456");
        assert_eq!(song.title, "Fixture Song - Part 1");
        assert_eq!(
            song.cover_url.as_deref(),
            Some("https://example.test/cover.jpg")
        );
    }

    #[test]
    fn selects_highest_dash_audio_from_playback_fixture() {
        let result = parse_playback(include_bytes!(
            "../../../../tests/fixtures/bilibili/playback.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.0, "https://example.test/audio-192.m4s");
        assert_eq!(result.1, "30280");
    }

    #[tokio::test]
    #[ignore = "requires live Bilibili service"]
    async fn resolves_live_resource() {
        let source = BilibiliSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .resolve_song(crate::domain::source::SongRef {
                source: "bilibili".into(),
                scope: "song".into(),
                site_id: "BV1Q541167Qg|168409646".into(),
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(result
            .unwrap_or_else(|_| unreachable!())
            .url
            .starts_with("http"));
    }

    #[tokio::test]
    #[ignore = "requires live Bilibili service"]
    async fn searches_live_service() {
        let source = BilibiliSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 5,
                sources: vec!["bilibili".into()],
            })
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }
}
