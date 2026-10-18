use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;

use crate::domain::collection::{Collection, CollectionType};
use crate::domain::lyrics::{merge_translation, parse_lrc};
use crate::domain::media::{Lyrics, PlayableResource};
use crate::domain::source::{
    AlbumRef, Artist, CollectionDetail, CollectionPage, MusicSource, SearchQuery, SearchResult,
    Song, SongRef, SourceCapabilities, SourceError,
};
use crate::infrastructure::credentials::SourceCookie;
use crate::infrastructure::http::{source_headers, HttpClient};

const SEARCH_API: &str = "https://c.y.qq.com/soso/fcgi-bin/search_for_qq_cp";
const PLAYBACK_API: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const LYRIC_API: &str = "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg";
const PLAYLIST_SEARCH_API: &str = "https://c.y.qq.com/soso/fcgi-bin/client_music_search_songlist";
const ALBUM_SEARCH_API: &str = "https://c.y.qq.com/soso/fcgi-bin/client_search_cp";
const PLAYLIST_DETAIL_API: &str = "https://c.y.qq.com/qzone/fcg-bin/fcg_ucc_getcdinfo_byids_cp.fcg";
const ALBUM_DETAIL_API: &str = "https://c.y.qq.com/v8/fcg-bin/fcg_v8_album_info_cp.fcg";
const COVER_TEMPLATE: &str = "https://y.gtimg.cn/music/photo_new/T002R300x300M000";
/// Fixed weak token the public web endpoints accept for an anonymous session.
/// The lyric/playlist CGIs need no per-request signature, so no signing scheme
/// has to be derived here.
const WEB_TOKEN: &str = "5381";
/// Stream filename prefixes, best quality first. `size_*` on a search hit
/// tells which tiers exist, but the vkey call simply fails for absent ones.
const FLAC_PREFIX: &str = "F000";
const MP3_320_PREFIX: &str = "M800";
const MP3_128_PREFIX: &str = "M500";

pub struct QqSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> QqSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for QqSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "qq"
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            song_search: true,
            playlist_search: true,
            album_search: true,
            lyrics: true,
            cover: true,
            cookie: true,
            qr_login: true,
            qualities: vec![
                MP3_128_PREFIX.into(),
                MP3_320_PREFIX.into(),
                FLAC_PREFIX.into(),
            ],
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let body = self
            .get(
                SEARCH_API,
                &[
                    ("w", query.query.clone()),
                    ("format", "json".into()),
                    ("p", query.page.to_string()),
                    ("n", query.page_size.to_string()),
                ],
            )
            .await?;
        parse_search(&body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        self.vkey_url(song, &[MP3_320_PREFIX, MP3_128_PREFIX]).await
    }

    /// Downloads try lossless first; playback skips it because a FLAC stream is
    /// wasteful for listening and usually needs a paid cookie.
    async fn download_url(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        self.vkey_url(song, &[FLAC_PREFIX, MP3_320_PREFIX, MP3_128_PREFIX])
            .await
    }

    async fn lyrics(&self, song: SongRef) -> Result<Lyrics, SourceError> {
        let mid = self.song_mid(&song)?;
        // `format=json` avoids the jsonp wrapper; `g_tk` is the weak web token.
        let body = self
            .get(
                LYRIC_API,
                &[
                    ("songmid", mid),
                    ("g_tk", WEB_TOKEN.into()),
                    ("format", "json".into()),
                    ("nobase64", "1".into()),
                    ("platform", "yqq".into()),
                ],
            )
            .await?;
        parse_lyrics(&body)
    }

    async fn search_playlists(&self, query: SearchQuery) -> Result<CollectionPage, SourceError> {
        let body = self
            .get(
                PLAYLIST_SEARCH_API,
                &[
                    ("query", query.query.clone()),
                    ("format", "json".into()),
                    ("page_no", query.page.saturating_sub(1).to_string()),
                    ("num_per_page", query.page_size.to_string()),
                    ("remoteplace", "txt.yqq.playlist".into()),
                ],
            )
            .await?;
        parse_playlist_search(&body, query.page, query.page_size)
    }

    async fn search_albums(&self, query: SearchQuery) -> Result<CollectionPage, SourceError> {
        // `t=8` switches the generic search CGI from songs to albums.
        let body = self
            .get(
                ALBUM_SEARCH_API,
                &[
                    ("w", query.query.clone()),
                    ("format", "json".into()),
                    ("t", "8".into()),
                    ("p", query.page.to_string()),
                    ("n", query.page_size.to_string()),
                ],
            )
            .await?;
        parse_album_search(&body, query.page, query.page_size)
    }

    async fn playlist_detail(&self, id: &str) -> Result<CollectionDetail, SourceError> {
        let id = collection_id(id, "playlist")?;
        let body = self
            .get(
                PLAYLIST_DETAIL_API,
                &[
                    ("type", "1".into()),
                    ("json", "1".into()),
                    ("utf8", "1".into()),
                    ("onlysong", "0".into()),
                    ("disstid", id),
                    ("format", "json".into()),
                ],
            )
            .await?;
        parse_playlist_detail(&body)
    }

    async fn album_detail(&self, id: &str) -> Result<CollectionDetail, SourceError> {
        let id = collection_id(id, "album")?;
        let body = self
            .get(
                ALBUM_DETAIL_API,
                &[("albummid", id), ("format", "json".into())],
            )
            .await?;
        parse_album_detail(&body)
    }
}

impl<C> QqSource<C>
where
    C: HttpClient,
{
    async fn get(&self, api: &str, params: &[(&str, String)]) -> Result<Vec<u8>, SourceError> {
        let mut url = reqwest::Url::parse(api)
            .map_err(|error| source_error("SOURCE_URL_INVALID", error.to_string(), false))?;
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in params {
                pairs.append_pair(key, value);
            }
        }
        let response = self
            .client
            .get_with_headers(
                url.as_str(),
                &source_headers(self.id(), &self.cookie.value()),
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("qq returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        Ok(response.body)
    }

    /// Requests one vkey per candidate tier; QQ answers with an empty `purl`
    /// for tiers the current cookie may not stream.
    async fn vkey_url(
        &self,
        song: SongRef,
        prefixes: &[&str],
    ) -> Result<PlayableResource, SourceError> {
        let mid = self.song_mid(&song)?;
        let filenames: Vec<String> = prefixes
            .iter()
            .map(|prefix| format!("{prefix}{mid}{mid}{}", stream_extension(prefix)))
            .collect();
        let mids: Vec<&str> = prefixes.iter().map(|_| mid.as_str()).collect();
        let types: Vec<u8> = prefixes.iter().map(|_| 0).collect();
        let body = serde_json::json!({
            "comm": {
                "cv": 4747474,
                "ct": 24,
                "format": "json",
                "platform": "yqq.json",
                "needNewCode": 1,
                "uin": 0
            },
            "req_1": {
                "module": "music.vkey.GetVkey",
                "method": "UrlGetVkey",
                "param": {
                    "guid": "1000000000",
                    "songmid": mids,
                    "songtype": types,
                    "uin": "0",
                    "loginflag": 1,
                    "platform": "20",
                    "filename": filenames
                }
            }
        });
        let response = self
            .client
            .post_json(
                PLAYBACK_API,
                &body,
                &source_headers(self.id(), &self.cookie.value()),
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("qq returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        let (url, quality) = parse_playback(&response.body, &filenames)?;
        Ok(PlayableResource {
            song,
            url,
            quality,
            duration_ms: None,
            expires_at: None,
        })
    }

    fn song_mid(&self, song: &SongRef) -> Result<String, SourceError> {
        if song.source != self.id() || song.scope != "song" || song.site_id.trim().is_empty() {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid QQ song reference".into(),
                false,
            ));
        }
        Ok(song.site_id.trim().to_owned())
    }
}

/// Lossless streams are `.flac`; every mp3 tier keeps the `.mp3` suffix.
fn stream_extension(prefix: &str) -> &'static str {
    if prefix == FLAC_PREFIX {
        ".flac"
    } else {
        ".mp3"
    }
}

fn cover_url(album_mid: &str) -> Option<String> {
    (!album_mid.is_empty()).then(|| format!("{COVER_TEMPLATE}{album_mid}.jpg"))
}

fn collection_id(id: &str, label: &str) -> Result<String, SourceError> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Err(source_error(
            "INVALID_ARGUMENT",
            format!("invalid QQ {label} id"),
            false,
        ));
    }
    Ok(trimmed.to_owned())
}

/// QQ returns the source lyric in `lyric` and the translation in `trans`, both
/// base64 unless `nobase64=1` was honoured, so both forms are accepted.
fn parse_lyrics(body: &[u8]) -> Result<Lyrics, SourceError> {
    let payload: LyricResponse = decode(strip_jsonp(body))?;
    let original = lyric_text(payload.lyric);
    let translated = lyric_text(payload.trans);
    let Some(original) = original else {
        return Err(source_error(
            "SOURCE_LYRICS_UNAVAILABLE",
            format!(
                "QQ returned no lyrics, retcode={}",
                payload.retcode.unwrap_or_default()
            ),
            false,
        ));
    };
    let mut lines = parse_lrc(&original);
    if let Some(translated) = translated.as_deref() {
        lines = merge_translation(lines, translated);
    }
    Ok(Lyrics {
        original: Some(original),
        translated,
        romanized: None,
        lines,
    })
}

fn lyric_text(value: Option<String>) -> Option<String> {
    let raw = value?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // A timestamped document is already plaintext; anything else is base64.
    if trimmed.starts_with('[') {
        return Some(trimmed.to_owned());
    }
    decode_base64_lyric(trimmed).or_else(|| Some(trimmed.to_owned()))
}

fn decode_base64_lyric(value: &str) -> Option<String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    let text = String::from_utf8(decoded).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The playlist and lyric CGIs wrap JSON in a `callback(...)` envelope unless
/// `format=json` is honoured, so the wrapper is stripped when present.
fn strip_jsonp(body: &[u8]) -> &[u8] {
    let Some(start) = body.iter().position(|byte| *byte == b'{') else {
        return body;
    };
    let Some(end) = body.iter().rposition(|byte| *byte == b'}') else {
        return body;
    };
    if end > start {
        &body[start..=end]
    } else {
        body
    }
}

fn parse_playlist_search(
    body: &[u8],
    page: u32,
    page_size: u32,
) -> Result<CollectionPage, SourceError> {
    let payload: PlaylistSearchResponse = decode(strip_jsonp(body))?;
    let items = payload
        .data
        .list
        .into_iter()
        .map(|entry| Collection {
            id: entry.dissid,
            source: "qq".into(),
            content_type: CollectionType::Playlist,
            name: entry.dissname,
            description: entry.introduction.filter(|text| !text.is_empty()),
            creator: entry.creator.map(|creator| creator.name),
            cover_url: entry.imgurl.filter(|url| !url.is_empty()),
            track_count: entry.song_count,
        })
        .collect();
    Ok(CollectionPage {
        items,
        total: payload.data.sum,
        page,
        page_size,
    })
}

fn parse_album_search(
    body: &[u8],
    page: u32,
    page_size: u32,
) -> Result<CollectionPage, SourceError> {
    let payload: AlbumSearchResponse = decode(strip_jsonp(body))?;
    let items = payload
        .data
        .album
        .list
        .into_iter()
        .map(|entry| Collection {
            id: entry.album_mid.clone(),
            source: "qq".into(),
            content_type: CollectionType::Album,
            name: entry.album_name,
            description: None,
            creator: entry.singers.into_iter().next().map(|singer| singer.name),
            cover_url: cover_url(&entry.album_mid),
            track_count: entry.song_count,
        })
        .collect();
    Ok(CollectionPage {
        items,
        total: payload.data.album.total,
        page,
        page_size,
    })
}

fn parse_playlist_detail(body: &[u8]) -> Result<CollectionDetail, SourceError> {
    let payload: PlaylistDetailResponse = decode(strip_jsonp(body))?;
    let playlist = payload.cdlist.into_iter().next().ok_or_else(|| {
        source_error(
            "SOURCE_COLLECTION_UNAVAILABLE",
            format!(
                "QQ playlist is unavailable, code={}",
                payload.code.unwrap_or_default()
            ),
            false,
        )
    })?;
    let songs: Vec<Song> = playlist.songlist.into_iter().map(map_detail_song).collect();
    Ok(CollectionDetail {
        collection: Collection {
            id: playlist.dissid,
            source: "qq".into(),
            content_type: CollectionType::Playlist,
            name: playlist.dissname,
            description: playlist.desc.filter(|text| !text.is_empty()),
            creator: playlist.creator.map(|creator| creator.name),
            cover_url: playlist.logo.filter(|url| !url.is_empty()),
            track_count: Some(songs.len() as u64),
        },
        songs,
    })
}

fn parse_album_detail(body: &[u8]) -> Result<CollectionDetail, SourceError> {
    let payload: AlbumDetailResponse = decode(strip_jsonp(body))?;
    let album = payload.data.ok_or_else(|| {
        source_error(
            "SOURCE_COLLECTION_UNAVAILABLE",
            format!(
                "QQ album is unavailable, code={}",
                payload.code.unwrap_or_default()
            ),
            false,
        )
    })?;
    let songs: Vec<Song> = album.list.into_iter().map(map_detail_song).collect();
    Ok(CollectionDetail {
        collection: Collection {
            id: album.album_mid.clone(),
            source: "qq".into(),
            content_type: CollectionType::Album,
            name: album.album_name,
            description: album.desc.filter(|text| !text.is_empty()),
            creator: album.singer_name.filter(|name| !name.is_empty()),
            cover_url: cover_url(&album.album_mid),
            track_count: Some(songs.len() as u64),
        },
        songs,
    })
}

/// Playlist and album track entries share one shape, distinct from the search
/// CGI's flat `songname`/`albummid` fields. Both spellings are accepted because
/// the playlist CGI mixes them across API versions.
fn map_detail_song(song: DetailSong) -> Song {
    let nested = song.album;
    let album_mid = nested
        .as_ref()
        .and_then(|album| album.mid.clone())
        .or(song.album_mid)
        .unwrap_or_default();
    let album_name = nested
        .and_then(|album| album.name)
        .or(song.album_name)
        .unwrap_or_default();
    let cover = cover_url(&album_mid);
    Song {
        reference: SongRef {
            source: "qq".into(),
            scope: "song".into(),
            site_id: song.song_mid,
        },
        title: song.song_name,
        artists: song
            .singers
            .into_iter()
            .map(|singer| Artist {
                id: None,
                name: singer.name,
            })
            .collect(),
        album: (!album_mid.is_empty() || !album_name.is_empty()).then(|| AlbumRef {
            id: (!album_mid.is_empty()).then_some(album_mid),
            name: album_name,
            source: Some("qq".into()),
        }),
        duration_ms: Some(song.interval.saturating_mul(1_000)),
        cover_url: cover,
    }
}

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, SourceError> {
    serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    let payload: SearchResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let songs = payload.data.song;
    let items = songs
        .list
        .into_iter()
        .filter(|song| song.pay.as_ref().is_none_or(|pay| pay.pay_play != 1))
        .map(|song| {
            let cover_url = (!song.album_mid.is_empty()).then(|| {
                format!(
                    "https://y.gtimg.cn/music/photo_new/T002R300x300M000{}.jpg",
                    song.album_mid
                )
            });
            Song {
                reference: SongRef {
                    source: "qq".into(),
                    scope: "song".into(),
                    site_id: song.song_mid,
                },
                title: song.song_name,
                artists: song
                    .singers
                    .into_iter()
                    .map(|artist| Artist {
                        id: None,
                        name: artist.name,
                    })
                    .collect(),
                album: Some(AlbumRef {
                    id: (!song.album_mid.is_empty()).then_some(song.album_mid),
                    name: song.album_name,
                    source: Some("qq".into()),
                }),
                duration_ms: Some(song.interval.saturating_mul(1000)),
                cover_url,
            }
        })
        .collect();
    Ok(SearchResult {
        items,
        total: songs.total,
        page,
        page_size,
    })
}

fn parse_playback(body: &[u8], filenames: &[String]) -> Result<(String, String), SourceError> {
    let payload: PlaybackResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    for expected in filenames {
        if let Some(item) = payload
            .request
            .data
            .urls
            .iter()
            .find(|item| item.filename == *expected && !item.purl.is_empty())
        {
            let url = format!("https://ws.stream.qqmusic.qq.com/{}", item.purl);
            return Ok((url, expected[..4].to_owned()));
        }
    }
    Err(source_error(
        "SOURCE_PLAYBACK_UNAVAILABLE",
        "QQ response contained no playable URL".into(),
        false,
    ))
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
    song: SearchSongPage,
}

#[derive(Deserialize)]
struct SearchSongPage {
    #[serde(default)]
    list: Vec<SearchSong>,
    #[serde(rename = "totalnum")]
    total: Option<u64>,
}

#[derive(Deserialize)]
struct SearchSong {
    #[serde(rename = "songname")]
    song_name: String,
    #[serde(rename = "songmid")]
    song_mid: String,
    #[serde(rename = "albumname")]
    album_name: String,
    #[serde(rename = "albummid")]
    album_mid: String,
    interval: u64,
    #[serde(rename = "singer", default)]
    singers: Vec<SearchSinger>,
    #[serde(default)]
    pay: Option<SearchPay>,
}

/// Purchase gating qq reports per hit.
///
/// `payplay == 1` marks a subscription-only stream: the vkey endpoint answers
/// with an empty `purl` for every quality tier.
#[derive(Deserialize)]
struct SearchPay {
    #[serde(rename = "payplay", default)]
    pay_play: i32,
}

#[derive(Deserialize)]
struct SearchSinger {
    name: String,
}

#[derive(Deserialize)]
struct LyricResponse {
    #[serde(default)]
    retcode: Option<i32>,
    #[serde(default)]
    lyric: Option<String>,
    #[serde(default)]
    trans: Option<String>,
}

#[derive(Deserialize)]
struct PlaylistSearchResponse {
    data: PlaylistSearchData,
}

#[derive(Deserialize)]
struct PlaylistSearchData {
    #[serde(default)]
    list: Vec<PlaylistSearchEntry>,
    #[serde(default)]
    sum: Option<u64>,
}

#[derive(Deserialize)]
struct PlaylistSearchEntry {
    dissid: String,
    dissname: String,
    #[serde(default)]
    introduction: Option<String>,
    #[serde(default)]
    creator: Option<PlaylistCreator>,
    #[serde(default)]
    imgurl: Option<String>,
    #[serde(rename = "song_count", default)]
    song_count: Option<u64>,
}

#[derive(Deserialize)]
struct PlaylistCreator {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct AlbumSearchResponse {
    data: AlbumSearchData,
}

#[derive(Deserialize)]
struct AlbumSearchData {
    album: AlbumSearchPage,
}

#[derive(Deserialize)]
struct AlbumSearchPage {
    #[serde(default)]
    list: Vec<AlbumSearchEntry>,
    #[serde(rename = "totalnum", default)]
    total: Option<u64>,
}

#[derive(Deserialize)]
struct AlbumSearchEntry {
    #[serde(rename = "albumMID", alias = "albummid")]
    album_mid: String,
    #[serde(rename = "albumName", alias = "albumname")]
    album_name: String,
    #[serde(rename = "singer_list", alias = "singer", default)]
    singers: Vec<SearchSinger>,
    #[serde(rename = "song_count", default)]
    song_count: Option<u64>,
}

#[derive(Deserialize)]
struct PlaylistDetailResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    cdlist: Vec<PlaylistDetailEntry>,
}

#[derive(Deserialize)]
struct PlaylistDetailEntry {
    dissid: String,
    dissname: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    logo: Option<String>,
    #[serde(default)]
    creator: Option<PlaylistCreator>,
    #[serde(default)]
    songlist: Vec<DetailSong>,
}

/// Track shape shared by playlist `songlist` and album `getSongInfo` entries:
/// both use nested `album`/`singer` objects rather than the flat search fields.
#[derive(Deserialize)]
struct DetailSong {
    #[serde(rename = "songmid", alias = "mid")]
    song_mid: String,
    #[serde(rename = "songname", alias = "name", alias = "title")]
    song_name: String,
    #[serde(default)]
    interval: u64,
    #[serde(rename = "singer", default)]
    singers: Vec<SearchSinger>,
    #[serde(default)]
    album: Option<DetailAlbum>,
    #[serde(rename = "albummid", default)]
    album_mid: Option<String>,
    #[serde(rename = "albumname", default)]
    album_name: Option<String>,
}

#[derive(Deserialize)]
struct DetailAlbum {
    #[serde(default)]
    mid: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct AlbumDetailResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    data: Option<AlbumDetailData>,
}

#[derive(Deserialize)]
struct AlbumDetailData {
    #[serde(rename = "mid", alias = "albummid")]
    album_mid: String,
    #[serde(rename = "name", alias = "albumname")]
    album_name: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(rename = "singername", default)]
    singer_name: Option<String>,
    #[serde(default)]
    list: Vec<DetailSong>,
}

#[derive(Deserialize)]
struct PlaybackResponse {
    #[serde(rename = "req_1")]
    request: PlaybackRequest,
}

#[derive(Deserialize)]
struct PlaybackRequest {
    data: PlaybackData,
}

#[derive(Deserialize)]
struct PlaybackData {
    #[serde(rename = "midurlinfo", default)]
    urls: Vec<PlaybackUrl>,
}

#[derive(Deserialize)]
struct PlaybackUrl {
    filename: String,
    #[serde(default)]
    purl: String,
}

#[cfg(test)]
mod tests {
    use super::{
        parse_album_detail, parse_album_search, parse_lyrics, parse_playback,
        parse_playlist_detail, parse_playlist_search, parse_search, QqSource,
    };
    use crate::domain::collection::CollectionType;
    use crate::domain::source::{MusicSource, SearchQuery};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    /// Base64 payloads, the shape QQ sends when `nobase64` is ignored. They
    /// decode to "[00:01.00]cue one\n[00:03.00]cue two" and the matching
    /// "translated one"/"translated two" track.
    const LYRICS_BASE64: &str = r#"{
        "retcode": 0,
        "lyric": "WzAwOjAxLjAwXWN1ZSBvbmUKWzAwOjAzLjAwXWN1ZSB0d28=",
        "trans": "WzAwOjAxLjAwXXRyYW5zbGF0ZWQgb25lClswMDowMy4wMF10cmFuc2xhdGVkIHR3bw=="
    }"#;

    const LYRICS_PLAIN_NO_TRANSLATION: &str = r#"{
        "retcode": 0,
        "lyric": "[00:05.00]single cue",
        "trans": ""
    }"#;

    const PLAYLIST_SEARCH: &str = r#"{
        "data": {
            "sum": 7,
            "list": [
                {
                    "dissid": "7654321",
                    "dissname": "Evening Set",
                    "introduction": "slow tracks",
                    "creator": {"name": "Curator"},
                    "imgurl": "https://example.test/list.jpg",
                    "song_count": 30
                }
            ]
        }
    }"#;

    const ALBUM_SEARCH: &str = r#"{
        "data": {
            "album": {
                "totalnum": 3,
                "list": [
                    {
                        "albumMID": "ALBMID1",
                        "albumName": "Second Album",
                        "singer_list": [{"name": "Singer A"}],
                        "song_count": 8
                    }
                ]
            }
        }
    }"#;

    const PLAYLIST_DETAIL: &str = r#"callback({
        "code": 0,
        "cdlist": [
            {
                "dissid": "7654321",
                "dissname": "Evening Set",
                "desc": "slow tracks",
                "logo": "https://example.test/list.jpg",
                "creator": {"name": "Curator"},
                "songlist": [
                    {
                        "songmid": "MIDONE",
                        "songname": "First Track",
                        "interval": 201,
                        "singer": [{"name": "Singer A"}],
                        "album": {"mid": "ALBMID1", "name": "Second Album"}
                    },
                    {
                        "songmid": "MIDTWO",
                        "songname": "Second Track",
                        "interval": 178,
                        "singer": [{"name": "Singer B"}],
                        "albummid": "ALBMID2",
                        "albumname": "Third Album"
                    }
                ]
            }
        ]
    })"#;

    const ALBUM_DETAIL: &str = r#"{
        "code": 0,
        "data": {
            "mid": "ALBMID1",
            "name": "Second Album",
            "singername": "Singer A",
            "list": [
                {
                    "songmid": "MIDONE",
                    "songname": "First Track",
                    "interval": 201,
                    "singer": [{"name": "Singer A"}]
                }
            ]
        }
    }"#;

    #[test]
    fn parses_search_fixture() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/qq/search.json"),
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.total, Some(1));
        assert_eq!(result.items[0].reference.site_id, "MID123");
        assert_eq!(result.items[0].duration_ms, Some(183_000));
    }

    #[test]
    fn search_drops_subscription_only_hits() {
        const MIXED: &[u8] = br#"{
            "data": {
                "song": {
                    "totalnum": 2,
                    "list": [
                        {
                            "songname": "Free Track",
                            "songmid": "FREE1",
                            "albumname": "Album",
                            "albummid": "ALB1",
                            "interval": 180,
                            "singer": [{ "name": "Artist" }],
                            "pay": { "payplay": 0 }
                        },
                        {
                            "songname": "VIP Track",
                            "songmid": "VIP1",
                            "albumname": "Album",
                            "albummid": "ALB1",
                            "interval": 200,
                            "singer": [{ "name": "Artist" }],
                            "pay": { "payplay": 1 }
                        }
                    ]
                }
            }
        }"#;
        let result =
            parse_search(MIXED, 1, 20).unwrap_or_else(|error| panic!("must parse: {error:?}"));
        // payplay == 1 means the vkey endpoint returns an empty purl for every
        // tier, so the hit is unusable without a paid cookie.
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].reference.site_id, "FREE1");
    }

    #[test]
    fn selects_first_requested_playback_quality() {
        let filenames = vec!["M800MIDMID.mp3".into(), "M500MIDMID.mp3".into()];
        let result = parse_playback(
            include_bytes!("../../../../tests/fixtures/qq/playback.json"),
            &filenames,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.0, "https://ws.stream.qqmusic.qq.com/stream-320.mp3");
        assert_eq!(result.1, "M800");
    }

    #[tokio::test]
    #[ignore = "requires live QQ Music service"]
    async fn resolves_live_resource() {
        let source = QqSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let search = source
            .search_songs(SearchQuery {
                // Even hits with `pay_play == 0` mostly answer with an empty
                // `purl`; roughly one row in six yields a vkey, so scan a wide
                // page instead of the first few.
                query: "千千阙歌".into(),
                page: 1,
                page_size: 30,
                sources: vec!["qq".into()],
            })
            .await
            .unwrap_or_else(|error| panic!("live search failed: {error:?}"));
        assert!(!search.items.is_empty(), "qq search returned no rows");
        let mut last_error = None;
        for item in search.items {
            match source.resolve_song(item.reference).await {
                Ok(resource) => {
                    assert!(resource
                        .url
                        .starts_with("https://ws.stream.qqmusic.qq.com/"));
                    return;
                }
                Err(error) => last_error = Some(error),
            }
        }
        panic!("no live search result resolved: {last_error:?}");
    }

    #[tokio::test]
    #[ignore = "requires live QQ Music service"]
    async fn searches_live_service() {
        let source = QqSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 5,
                sources: vec!["qq".into()],
            })
            .await;
        assert!(result.is_ok());
        assert!(!result.unwrap_or_else(|_| unreachable!()).items.is_empty());
    }

    #[test]
    fn decodes_base64_lyrics_and_merges_translation() {
        let lyrics = parse_lyrics(LYRICS_BASE64.as_bytes()).unwrap_or_else(|_| unreachable!());
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].start_ms, Some(1_000));
        assert_eq!(lyrics.lines[0].text, "cue one\ntranslated one");
        assert_eq!(lyrics.lines[1].text, "cue two\ntranslated two");
        assert!(lyrics
            .original
            .as_deref()
            .is_some_and(|text| text.starts_with("[00:01.00]")));
    }

    #[test]
    fn accepts_plaintext_lyrics_without_translation() {
        let lyrics =
            parse_lyrics(LYRICS_PLAIN_NO_TRANSLATION.as_bytes()).unwrap_or_else(|_| unreachable!());
        assert_eq!(lyrics.translated, None);
        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(lyrics.lines[0].text, "single cue");
        assert_eq!(lyrics.lines[0].start_ms, Some(5_000));
    }

    #[test]
    fn reports_missing_lyrics_as_unavailable() {
        let error = parse_lyrics(br#"{"retcode": -1901, "lyric": ""}"#)
            .err()
            .unwrap_or_else(|| unreachable!());
        assert_eq!(error.code, "SOURCE_LYRICS_UNAVAILABLE");
        assert!(error.message.contains("-1901"));
        assert!(!error.retryable);
    }

    #[test]
    fn maps_playlist_search_entries() {
        let page = parse_playlist_search(PLAYLIST_SEARCH.as_bytes(), 1, 20)
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(page.total, Some(7));
        assert_eq!(page.items[0].id, "7654321");
        assert_eq!(page.items[0].creator.as_deref(), Some("Curator"));
        assert_eq!(page.items[0].track_count, Some(30));
        assert!(matches!(
            page.items[0].content_type,
            CollectionType::Playlist
        ));
    }

    #[test]
    fn maps_album_search_entries_with_derived_cover() {
        let page =
            parse_album_search(ALBUM_SEARCH.as_bytes(), 1, 20).unwrap_or_else(|_| unreachable!());
        assert_eq!(page.total, Some(3));
        assert_eq!(page.items[0].id, "ALBMID1");
        assert_eq!(page.items[0].creator.as_deref(), Some("Singer A"));
        assert_eq!(
            page.items[0].cover_url.as_deref(),
            Some("https://y.gtimg.cn/music/photo_new/T002R300x300M000ALBMID1.jpg")
        );
    }

    #[test]
    fn returns_empty_page_for_zero_result_search() {
        let playlists = parse_playlist_search(br#"{"data": {"sum": 0, "list": []}}"#, 1, 20)
            .unwrap_or_else(|_| unreachable!());
        assert!(playlists.items.is_empty());
        assert_eq!(playlists.total, Some(0));
        let albums = parse_album_search(
            br#"{"data": {"album": {"totalnum": 0, "list": []}}}"#,
            1,
            20,
        )
        .unwrap_or_else(|_| unreachable!());
        assert!(albums.items.is_empty());
    }

    #[test]
    fn maps_playlist_detail_through_jsonp_wrapper() {
        let detail =
            parse_playlist_detail(PLAYLIST_DETAIL.as_bytes()).unwrap_or_else(|_| unreachable!());
        assert_eq!(detail.collection.name, "Evening Set");
        assert_eq!(detail.collection.track_count, Some(2));
        assert_eq!(detail.songs.len(), 2);
        assert_eq!(detail.songs[0].reference.site_id, "MIDONE");
        assert_eq!(detail.songs[0].duration_ms, Some(201_000));
        // Nested `album` object and the flat `albummid` form must both map.
        assert_eq!(
            detail.songs[0].album.as_ref().and_then(|a| a.id.as_deref()),
            Some("ALBMID1")
        );
        assert_eq!(
            detail.songs[1].album.as_ref().map(|a| a.name.as_str()),
            Some("Third Album")
        );
        assert_eq!(detail.songs[1].artists[0].name, "Singer B");
    }

    #[test]
    fn maps_album_detail_tracks() {
        let detail = parse_album_detail(ALBUM_DETAIL.as_bytes()).unwrap_or_else(|_| unreachable!());
        assert_eq!(detail.collection.id, "ALBMID1");
        assert_eq!(detail.collection.creator.as_deref(), Some("Singer A"));
        assert!(matches!(
            detail.collection.content_type,
            CollectionType::Album
        ));
        assert_eq!(detail.songs.len(), 1);
        assert_eq!(detail.songs[0].reference.site_id, "MIDONE");
    }

    #[test]
    fn maps_absent_collection_to_source_error() {
        let error = parse_playlist_detail(br#"{"code": 2000, "cdlist": []}"#)
            .err()
            .unwrap_or_else(|| unreachable!());
        assert_eq!(error.code, "SOURCE_COLLECTION_UNAVAILABLE");
        assert!(error.message.contains("2000"));
        assert!(!error.retryable);
    }

    #[test]
    fn maps_malformed_payload_to_response_invalid() {
        let error = parse_album_search(b"not json at all", 1, 20)
            .err()
            .unwrap_or_else(|| unreachable!());
        assert_eq!(error.code, "SOURCE_RESPONSE_INVALID");
        assert!(!error.retryable);
    }

    #[test]
    fn download_prefers_lossless_over_playback_tier() {
        // Download asks for F000 first; playback starts at M800.
        let flac = vec!["F000MIDMID.flac".to_owned(), "M800MIDMID.mp3".to_owned()];
        let payload = br#"{"req_1": {"data": {"midurlinfo": [
            {"filename": "F000MIDMID.flac", "purl": "stream.flac"},
            {"filename": "M800MIDMID.mp3", "purl": "stream-320.mp3"}
        ]}}}"#;
        let (url, quality) = parse_playback(payload, &flac).unwrap_or_else(|_| unreachable!());
        assert_eq!(url, "https://ws.stream.qqmusic.qq.com/stream.flac");
        assert_eq!(quality, "F000");
    }

    #[test]
    fn advertises_the_capabilities_it_implements() {
        let capabilities =
            QqSource::new(ReqwestHttpClient::default(), SourceCookie::empty()).capabilities();
        assert!(capabilities.playlist_search);
        assert!(capabilities.album_search);
        assert!(capabilities.lyrics);
        assert!(capabilities.qr_login);
        assert!(capabilities.qualities.contains(&"F000".to_owned()));
    }

    #[tokio::test]
    #[ignore = "requires live QQ Music service"]
    async fn fetches_live_album_detail() {
        let source = QqSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let detail = source
            .album_detail("002fRO0N4FaUWU")
            .await
            .unwrap_or_else(|error| panic!("live album detail failed: {error:?}"));
        assert!(!detail.songs.is_empty());
    }
}
