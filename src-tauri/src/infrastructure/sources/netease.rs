use aes::cipher::{block_padding::Pkcs7, BlockEncrypt, BlockEncryptMut, KeyInit, KeyIvInit};
use aes::Aes128;
use async_trait::async_trait;
use base64::Engine;
use num_bigint::BigUint;
use rand::{distributions::Alphanumeric, Rng};
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

const FORWARD_API: &str = "http://music.163.com/api/linux/forward";
const PLAYBACK_API: &str = "http://music.163.com/weapi/song/enhance/player/url";
const SEARCH_URL: &str = "http://music.163.com/api/cloudsearch/pc";
const LYRIC_URL: &str = "http://music.163.com/api/song/lyric";
const PLAYLIST_URL: &str = "http://music.163.com/api/v6/playlist/detail";
/// Netease `br` tiers accepted by the player URL endpoint.
const QUALITY_TIERS: [u32; 4] = [128_000, 192_000, 320_000, 999_000];
const LINUX_KEY: [u8; 16] = *b"rFgB&h#%2?^eDg:Q";
const WEAPI_NONCE: &str = "0CoJUm6Qyw8W8jud";
const WEAPI_IV: &str = "0102030405060708";
const WEAPI_MODULUS: &str = "00e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b725152b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbda92557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe4875d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7";

pub struct NeteaseSource<C> {
    client: C,
    cookie: SourceCookie,
}

impl<C> NeteaseSource<C> {
    pub fn new(client: C, cookie: SourceCookie) -> Self {
        Self { client, cookie }
    }
}

#[async_trait]
impl<C> MusicSource for NeteaseSource<C>
where
    C: HttpClient,
{
    fn id(&self) -> &str {
        "netease"
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
            qualities: QUALITY_TIERS.iter().map(u32::to_string).collect(),
        }
    }

    async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
        let body = self.forward(SEARCH_URL, search_params(&query, 1)).await?;
        parse_search(&body, query.page, query.page_size)
    }

    async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        self.player_url(song, 320_000).await
    }

    /// Downloads ask for the top tier; playback stays at 320k to keep the
    /// stream cheap for sources that meter bandwidth.
    async fn download_url(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
        self.player_url(song, 999_000).await
    }

    async fn lyrics(&self, song: SongRef) -> Result<Lyrics, SourceError> {
        let id = self.song_id(&song)?;
        let body = self
            .forward(
                LYRIC_URL,
                serde_json::json!({"id": id, "lv": -1, "tv": -1, "rv": -1, "kv": -1}),
            )
            .await?;
        parse_lyrics(&body)
    }

    async fn search_playlists(&self, query: SearchQuery) -> Result<CollectionPage, SourceError> {
        // cloudsearch type 1000 = playlists, 10 = albums.
        let body = self
            .forward(SEARCH_URL, search_params(&query, 1000))
            .await?;
        parse_playlist_search(&body, query.page, query.page_size)
    }

    async fn search_albums(&self, query: SearchQuery) -> Result<CollectionPage, SourceError> {
        let body = self.forward(SEARCH_URL, search_params(&query, 10)).await?;
        parse_album_search(&body, query.page, query.page_size)
    }

    async fn playlist_detail(&self, id: &str) -> Result<CollectionDetail, SourceError> {
        let id = collection_id(id, "playlist")?;
        let body = self
            .forward(
                PLAYLIST_URL,
                serde_json::json!({"id": id, "n": 1000, "s": 0}),
            )
            .await?;
        parse_playlist_detail(&body)
    }

    async fn album_detail(&self, id: &str) -> Result<CollectionDetail, SourceError> {
        let id = collection_id(id, "album")?;
        // The album endpoint keys off the path, not a parameter.
        let body = self
            .forward(
                &format!("http://music.163.com/api/v1/album/{id}"),
                serde_json::json!({}),
            )
            .await?;
        parse_album_detail(&body)
    }
}

impl<C> NeteaseSource<C>
where
    C: HttpClient,
{
    /// Posts an encrypted request through the Linux client forwarder, which
    /// proxies any `music.163.com/api` path without a per-endpoint signature.
    async fn forward(&self, url: &str, params: serde_json::Value) -> Result<Vec<u8>, SourceError> {
        let payload = serde_json::json!({
            "method": "POST",
            "url": url,
            "params": params
        });
        let encrypted = encrypt_linux(payload.to_string().as_bytes());
        let response = self
            .client
            .post_form(
                FORWARD_API,
                &[("eparams", encrypted)],
                &source_headers(self.id(), &self.cookie.value()),
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("netease returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        Ok(response.body)
    }

    async fn player_url(
        &self,
        song: SongRef,
        bitrate: u32,
    ) -> Result<PlayableResource, SourceError> {
        let id = self.song_id(&song)?;
        let payload = serde_json::json!({"ids": [id], "br": bitrate}).to_string();
        let secret: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(16)
            .map(char::from)
            .collect();
        let (params, enc_sec_key) = encrypt_weapi(&payload, &secret)?;
        let response = self
            .client
            .post_form(
                PLAYBACK_API,
                &[("params", params), ("encSecKey", enc_sec_key)],
                &source_headers(self.id(), &self.cookie.value()),
            )
            .await
            .map_err(|error| source_error("SOURCE_NETWORK_ERROR", error.to_string(), true))?;
        if !(200..300).contains(&response.status) {
            return Err(source_error(
                "SOURCE_HTTP_ERROR",
                format!("netease returned HTTP {}", response.status),
                response.status >= 500,
            ));
        }
        let playback = parse_playback(&response.body)?;
        Ok(PlayableResource {
            song,
            url: playback.url,
            quality: playback.bitrate.to_string(),
            duration_ms: None,
            expires_at: None,
        })
    }

    fn song_id(&self, song: &SongRef) -> Result<String, SourceError> {
        if song.source != self.id() || song.scope != "song" || song.site_id.parse::<u64>().is_err()
        {
            return Err(source_error(
                "INVALID_ARGUMENT",
                "invalid Netease song reference".into(),
                false,
            ));
        }
        Ok(song.site_id.clone())
    }
}

fn encrypt_linux(input: &[u8]) -> String {
    let padding = 16 - input.len() % 16;
    let mut data = input.to_vec();
    data.extend(std::iter::repeat_n(padding as u8, padding));
    let cipher = Aes128::new(&LINUX_KEY.into());
    for chunk in data.chunks_exact_mut(16) {
        cipher.encrypt_block(chunk.into());
    }
    hex::encode_upper(data)
}

fn encrypt_weapi(text: &str, secret: &str) -> Result<(String, String), SourceError> {
    if secret.len() != 16 || !secret.is_ascii() {
        return Err(source_error(
            "SOURCE_ENCRYPTION_ERROR",
            "Netease WeAPI secret must be 16 ASCII bytes".into(),
            false,
        ));
    }
    let first = aes_cbc_base64(text, WEAPI_NONCE)?;
    let params = aes_cbc_base64(&first, secret)?;
    let reversed = secret.bytes().rev().collect::<Vec<_>>();
    let value = BigUint::from_bytes_be(&reversed);
    let exponent = BigUint::from(65_537_u32);
    let modulus = BigUint::parse_bytes(WEAPI_MODULUS.as_bytes(), 16).ok_or_else(|| {
        source_error(
            "SOURCE_ENCRYPTION_ERROR",
            "invalid Netease RSA modulus".into(),
            false,
        )
    })?;
    let enc_sec_key = format!("{:0>256x}", value.modpow(&exponent, &modulus));
    Ok((params, enc_sec_key))
}

fn aes_cbc_base64(text: &str, key: &str) -> Result<String, SourceError> {
    let mut buffer = text.as_bytes().to_vec();
    let message_len = buffer.len();
    buffer.resize(message_len + 16, 0);
    let encryptor = cbc::Encryptor::<Aes128>::new_from_slices(key.as_bytes(), WEAPI_IV.as_bytes())
        .map_err(|error| source_error("SOURCE_ENCRYPTION_ERROR", error.to_string(), false))?;
    let encrypted = encryptor
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, message_len)
        .map_err(|error| source_error("SOURCE_ENCRYPTION_ERROR", error.to_string(), false))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(encrypted))
}

fn parse_playback(body: &[u8]) -> Result<Playback, SourceError> {
    let payload: PlaybackResponse = serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))?;
    let item = payload.data.into_iter().next().ok_or_else(|| {
        source_error(
            "SOURCE_PLAYBACK_UNAVAILABLE",
            "Netease response contained no playback item".into(),
            false,
        )
    })?;
    let url = item.url.filter(|url| is_http_url(url)).ok_or_else(|| {
        source_error(
            "SOURCE_PLAYBACK_UNAVAILABLE",
            format!("Netease playback unavailable with code {}", item.code),
            false,
        )
    })?;
    if item.code != 200 {
        return Err(source_error(
            "SOURCE_PLAYBACK_UNAVAILABLE",
            format!("Netease playback unavailable with code {}", item.code),
            false,
        ));
    }
    Ok(Playback {
        url,
        bitrate: item.bitrate,
    })
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

/// cloudsearch params are identical across song/album/playlist; only `type`
/// selects the entity.
fn search_params(query: &SearchQuery, entity_type: u32) -> serde_json::Value {
    serde_json::json!({
        "s": query.query,
        "type": entity_type,
        "offset": query.page.saturating_sub(1).saturating_mul(query.page_size),
        "limit": query.page_size
    })
}

fn collection_id(id: &str, label: &str) -> Result<String, SourceError> {
    let trimmed = id.trim();
    if trimmed.is_empty() || trimmed.parse::<u64>().is_err() {
        return Err(source_error(
            "INVALID_ARGUMENT",
            format!("invalid Netease {label} id"),
            false,
        ));
    }
    Ok(trimmed.to_owned())
}

fn map_song(song: SearchSong) -> Song {
    Song {
        reference: SongRef {
            source: "netease".into(),
            scope: "song".into(),
            site_id: song.id.to_string(),
        },
        title: song.name,
        artists: song
            .artists
            .into_iter()
            .map(|artist| Artist {
                id: artist.id.map(|value| value.to_string()),
                name: artist.name,
            })
            .collect(),
        album: Some(AlbumRef {
            id: song.album.id.map(|value| value.to_string()),
            name: song.album.name,
            source: Some("netease".into()),
        }),
        duration_ms: Some(song.duration_ms),
        cover_url: song.album.pic_url,
    }
}

fn parse_search(body: &[u8], page: u32, page_size: u32) -> Result<SearchResult, SourceError> {
    let payload: SearchResponse = decode(body)?;
    Ok(SearchResult {
        items: payload
            .result
            .songs
            .into_iter()
            .filter(is_anonymously_playable)
            .map(map_song)
            .collect(),
        total: payload.result.song_count,
        page,
        page_size,
    })
}

/// Whether an anonymous caller can stream the hit at all.
///
/// Hits without a `privilege` block are kept: absence means the endpoint did not
/// report entitlement, not that playback is denied.
fn is_anonymously_playable(song: &SearchSong) -> bool {
    song.privilege
        .as_ref()
        .is_none_or(|privilege| privilege.fl > 0)
}

fn parse_playlist_search(
    body: &[u8],
    page: u32,
    page_size: u32,
) -> Result<CollectionPage, SourceError> {
    let payload: SearchResponse = decode(body)?;
    let items = payload
        .result
        .playlists
        .into_iter()
        .map(|playlist| Collection {
            id: playlist.id.to_string(),
            source: "netease".into(),
            content_type: CollectionType::Playlist,
            name: playlist.name,
            description: playlist.description.filter(|text| !text.is_empty()),
            creator: playlist.creator.map(|creator| creator.nickname),
            cover_url: playlist.cover_url.filter(|url| !url.is_empty()),
            track_count: playlist.track_count,
        })
        .collect();
    Ok(CollectionPage {
        items,
        total: payload.result.playlist_count,
        page,
        page_size,
    })
}

fn parse_album_search(
    body: &[u8],
    page: u32,
    page_size: u32,
) -> Result<CollectionPage, SourceError> {
    let payload: SearchResponse = decode(body)?;
    let items = payload
        .result
        .albums
        .into_iter()
        .map(|album| Collection {
            id: album.id.to_string(),
            source: "netease".into(),
            content_type: CollectionType::Album,
            name: album.name,
            description: album.description.filter(|text| !text.is_empty()),
            creator: album.artist.map(|artist| artist.name),
            cover_url: album.pic_url.filter(|url| !url.is_empty()),
            track_count: album.size,
        })
        .collect();
    Ok(CollectionPage {
        items,
        total: payload.result.album_count,
        page,
        page_size,
    })
}

fn parse_playlist_detail(body: &[u8]) -> Result<CollectionDetail, SourceError> {
    let payload: PlaylistDetailResponse = decode(body)?;
    let playlist = payload.playlist.ok_or_else(|| {
        source_error(
            "SOURCE_COLLECTION_UNAVAILABLE",
            format!(
                "Netease playlist is unavailable, code={}",
                payload.code.unwrap_or_default()
            ),
            false,
        )
    })?;
    let songs: Vec<Song> = playlist.tracks.into_iter().map(map_song).collect();
    Ok(CollectionDetail {
        collection: Collection {
            id: playlist.id.to_string(),
            source: "netease".into(),
            content_type: CollectionType::Playlist,
            name: playlist.name,
            description: playlist.description.filter(|text| !text.is_empty()),
            creator: playlist.creator.map(|creator| creator.nickname),
            cover_url: playlist.cover_url.filter(|url| !url.is_empty()),
            track_count: playlist.track_count.or(Some(songs.len() as u64)),
        },
        songs,
    })
}

fn parse_album_detail(body: &[u8]) -> Result<CollectionDetail, SourceError> {
    let payload: AlbumDetailResponse = decode(body)?;
    let album = payload.album.ok_or_else(|| {
        source_error(
            "SOURCE_COLLECTION_UNAVAILABLE",
            format!(
                "Netease album is unavailable, code={}",
                payload.code.unwrap_or_default()
            ),
            false,
        )
    })?;
    let songs: Vec<Song> = payload.songs.into_iter().map(map_song).collect();
    Ok(CollectionDetail {
        collection: Collection {
            id: album.id.to_string(),
            source: "netease".into(),
            content_type: CollectionType::Album,
            name: album.name,
            description: album.description.filter(|text| !text.is_empty()),
            creator: album.artist.map(|artist| artist.name),
            cover_url: album.pic_url.filter(|url| !url.is_empty()),
            track_count: album.size.or(Some(songs.len() as u64)),
        },
        songs,
    })
}

/// Netease returns the source lyric in `lrc` and the translation in `tlyric`,
/// with `romalrc` carrying romaji for Japanese tracks.
fn parse_lyrics(body: &[u8]) -> Result<Lyrics, SourceError> {
    let payload: LyricResponse = decode(body)?;
    let original = payload.lrc.and_then(|lrc| lrc.into_text());
    let translated = payload.tlyric.and_then(|lrc| lrc.into_text());
    let romanized = payload.romalrc.and_then(|lrc| lrc.into_text());
    let Some(original) = original else {
        return Err(source_error(
            "SOURCE_LYRICS_UNAVAILABLE",
            format!(
                "Netease returned no lyrics, code={}",
                payload.code.unwrap_or_default()
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
        romanized,
        lines,
    })
}

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, SourceError> {
    serde_json::from_slice(body)
        .map_err(|error| source_error("SOURCE_RESPONSE_INVALID", error.to_string(), false))
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
    result: SearchResponseResult,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponseResult {
    #[serde(default)]
    songs: Vec<SearchSong>,
    song_count: Option<u64>,
    #[serde(default)]
    playlists: Vec<SearchPlaylist>,
    playlist_count: Option<u64>,
    #[serde(default)]
    albums: Vec<SearchAlbumEntry>,
    album_count: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchPlaylist {
    id: u64,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    creator: Option<PlaylistCreator>,
    #[serde(default)]
    cover_url: Option<String>,
    #[serde(default)]
    track_count: Option<u64>,
}

#[derive(Deserialize)]
struct PlaylistCreator {
    #[serde(default)]
    nickname: String,
}

/// Album entity as returned by cloudsearch type 10 and `/v1/album`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchAlbumEntry {
    id: u64,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    artist: Option<SearchArtist>,
    #[serde(default)]
    pic_url: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Deserialize)]
struct PlaylistDetailResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    playlist: Option<PlaylistDetail>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistDetail {
    id: u64,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    creator: Option<PlaylistCreator>,
    #[serde(default)]
    cover_url: Option<String>,
    #[serde(default)]
    track_count: Option<u64>,
    #[serde(default)]
    tracks: Vec<SearchSong>,
}

#[derive(Deserialize)]
struct AlbumDetailResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    album: Option<SearchAlbumEntry>,
    #[serde(default)]
    songs: Vec<SearchSong>,
}

#[derive(Deserialize)]
struct LyricResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    lrc: Option<LyricBody>,
    #[serde(default)]
    tlyric: Option<LyricBody>,
    #[serde(default)]
    romalrc: Option<LyricBody>,
}

#[derive(Deserialize)]
struct LyricBody {
    #[serde(default)]
    lyric: Option<String>,
}

impl LyricBody {
    /// Netease sends `{"lyric": ""}` for tracks with no track of that kind.
    fn into_text(self) -> Option<String> {
        self.lyric
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    }
}

#[derive(Deserialize)]
struct SearchSong {
    id: u64,
    name: String,
    #[serde(rename = "ar", default)]
    artists: Vec<SearchArtist>,
    #[serde(rename = "al")]
    album: SearchAlbum,
    #[serde(rename = "dt")]
    duration_ms: u64,
    #[serde(default)]
    privilege: Option<SearchPrivilege>,
}

/// Playback entitlement netease attaches to every cloudsearch hit.
///
/// `fl` is the bitrate the *current* account may stream; zero means the track is
/// unplayable without a subscription.
#[derive(Deserialize)]
struct SearchPrivilege {
    #[serde(default)]
    fl: u64,
}

#[derive(Deserialize)]
struct SearchArtist {
    id: Option<u64>,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchAlbum {
    id: Option<u64>,
    name: String,
    pic_url: Option<String>,
}

#[derive(Deserialize)]
struct PlaybackResponse {
    #[serde(default)]
    data: Vec<PlaybackItem>,
}

#[derive(Deserialize)]
struct PlaybackItem {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    code: i32,
    #[serde(rename = "br", default)]
    bitrate: u32,
}

struct Playback {
    url: String,
    bitrate: u32,
}

#[cfg(test)]
mod tests {
    use super::{
        encrypt_linux, encrypt_weapi, parse_album_detail, parse_album_search, parse_lyrics,
        parse_playback, parse_playlist_detail, parse_playlist_search, parse_search, NeteaseSource,
    };
    use crate::domain::collection::CollectionType;
    use crate::domain::source::{MusicSource, SearchQuery};
    use crate::infrastructure::credentials::SourceCookie;
    use crate::infrastructure::http::ReqwestHttpClient;

    const LYRICS_WITH_TRANSLATION: &str = r#"{
        "code": 200,
        "lrc": {"lyric": "[00:01.00]first cue\n[00:04.50]second cue\n"},
        "tlyric": {"lyric": "[00:01.00]第一句\n[00:04.50]第二句\n"},
        "romalrc": {"lyric": ""}
    }"#;

    const LYRICS_WITHOUT_TRANSLATION: &str = r#"{
        "code": 200,
        "lrc": {"lyric": "[00:02.00]only cue\n"},
        "tlyric": {"lyric": ""}
    }"#;

    const PLAYLIST_SEARCH: &str = r#"{
        "result": {
            "playlistCount": 2,
            "playlists": [
                {
                    "id": 900,
                    "name": "Focus Mix",
                    "description": "steady beats",
                    "creator": {"nickname": "DJ Test"},
                    "coverImgUrl": "https://example.test/p.jpg",
                    "trackCount": 42
                }
            ]
        }
    }"#;

    const ALBUM_SEARCH: &str = r#"{
        "result": {
            "albumCount": 1,
            "albums": [
                {
                    "id": 700,
                    "name": "Test Album",
                    "artist": {"id": 5, "name": "Artist A"},
                    "picUrl": "https://example.test/a.jpg",
                    "size": 10
                }
            ]
        }
    }"#;

    const PLAYLIST_DETAIL: &str = r#"{
        "code": 200,
        "playlist": {
            "id": 900,
            "name": "Focus Mix",
            "creator": {"nickname": "DJ Test"},
            "coverImgUrl": "https://example.test/p.jpg",
            "trackCount": 2,
            "tracks": [
                {
                    "id": 111,
                    "name": "Track One",
                    "ar": [{"id": 1, "name": "Artist A"}],
                    "al": {"id": 700, "name": "Test Album", "picUrl": "https://example.test/a.jpg"},
                    "dt": 210000
                },
                {
                    "id": 222,
                    "name": "Track Two",
                    "ar": [{"id": 2, "name": "Artist B"}],
                    "al": {"id": 701, "name": "Other Album", "picUrl": ""},
                    "dt": 195000
                }
            ]
        }
    }"#;

    const ALBUM_DETAIL: &str = r#"{
        "code": 200,
        "album": {
            "id": 700,
            "name": "Test Album",
            "artist": {"id": 5, "name": "Artist A"},
            "picUrl": "https://example.test/a.jpg",
            "size": 1
        },
        "songs": [
            {
                "id": 111,
                "name": "Track One",
                "ar": [{"id": 5, "name": "Artist A"}],
                "al": {"id": 700, "name": "Test Album", "picUrl": "https://example.test/a.jpg"},
                "dt": 210000
            }
        ]
    }"#;

    #[test]
    fn linux_encryption_matches_reference_vector() {
        assert_eq!(encrypt_linux(b"{}"), "FA2B50F4B4B3202B91408D3B10E83D55");
    }

    #[test]
    fn weapi_encryption_matches_reference_vector() {
        let result = encrypt_weapi(r#"{"ids":["123"],"br":320000}"#, "abcdefghijklmnop")
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(
            result.0,
            "ahwgqngz7+zSdJWwdA5TPbL4YWjU3Agkro2fYFtQ0dud1fAAuQs9T2O4l5ZBpnAN"
        );
        assert_eq!(result.1, "d15a1683c992095d0c234c19966605c5c5964911268bbeda8cb8d08d834913e59d53b32358903a121b5fca784c1f5ae44951fd02524df58ecc98e52cc7cf8689b42c2e93ddf05b0592512d87f5960467e2f086c018849d76014d323500e30f13ef4cafbb0cf5a66731a3f1776c75ca35d0062dac70a3e33245afabcf47938487");
    }

    #[test]
    fn parses_playback_fixture() {
        let result = parse_playback(include_bytes!(
            "../../../../tests/fixtures/netease/playback.json"
        ))
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(result.url, "https://example.test/song.mp3");
        assert_eq!(result.bitrate, 320_000);
    }

    #[tokio::test]
    #[ignore = "requires live Netease service"]
    async fn resolves_live_resource() {
        let source = NeteaseSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let search = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                // Popular queries return mostly subscription-only masters, and
                // the search filter drops those, so a small page can legally
                // come back empty. Ask for enough rows to reach a free hit.
                page_size: 30,
                sources: vec!["netease".into()],
            })
            .await
            .unwrap_or_else(|error| panic!("live search failed: {error:?}"));
        assert!(
            !search.items.is_empty(),
            "no streamable hit survived the entitlement filter"
        );
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

    #[test]
    fn search_drops_hits_the_account_cannot_stream() {
        let result = parse_search(
            include_bytes!("../../../../tests/fixtures/netease/search.json"),
            1,
            20,
        )
        .unwrap_or_else(|error| panic!("fixture must parse: {error:?}"));
        // The fixture holds two hits; the second carries `privilege.fl == 0`,
        // which netease uses to mark a stream the caller may not play.
        assert_eq!(result.total, Some(2), "reported total stays untouched");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].reference.site_id, "123");
        assert_eq!(result.items[0].artists[0].name, "Artist A");
    }

    #[test]
    fn search_keeps_hits_without_entitlement_data() {
        const NO_PRIVILEGE: &[u8] = br#"{
            "result": {
                "songCount": 1,
                "songs": [
                    {
                        "id": 321,
                        "name": "Legacy Shape",
                        "ar": [{ "id": 1, "name": "Artist C" }],
                        "al": { "id": 2, "name": "Album C", "picUrl": null },
                        "dt": 120000
                    }
                ]
            }
        }"#;
        let result = parse_search(NO_PRIVILEGE, 1, 20)
            .unwrap_or_else(|error| panic!("fixture must parse: {error:?}"));
        assert_eq!(
            result.items.len(),
            1,
            "a missing privilege block must not be read as a denial"
        );
    }

    #[tokio::test]
    #[ignore = "requires live Netease service"]
    async fn searches_live_service() {
        let source = NeteaseSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let result = source
            .search_songs(SearchQuery {
                query: "周杰伦".into(),
                page: 1,
                page_size: 5,
                sources: vec!["netease".into()],
            })
            .await;
        assert!(result.is_ok());
        let result = result.unwrap_or_else(|_| unreachable!());
        assert!(
            !result.items.is_empty(),
            "live search returned no playable items; total={:?}",
            result.total
        );
    }

    #[test]
    fn merges_translation_onto_matching_lyric_cues() {
        let lyrics =
            parse_lyrics(LYRICS_WITH_TRANSLATION.as_bytes()).unwrap_or_else(|_| unreachable!());
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].start_ms, Some(1_000));
        assert_eq!(lyrics.lines[0].end_ms, Some(4_500));
        assert_eq!(lyrics.lines[0].text, "first cue\n第一句");
        assert_eq!(lyrics.lines[1].text, "second cue\n第二句");
        // An empty romalrc must not surface as an empty string.
        assert_eq!(lyrics.romanized, None);
    }

    #[test]
    fn keeps_lyrics_untranslated_when_platform_sends_none() {
        let lyrics =
            parse_lyrics(LYRICS_WITHOUT_TRANSLATION.as_bytes()).unwrap_or_else(|_| unreachable!());
        assert_eq!(lyrics.translated, None);
        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(lyrics.lines[0].text, "only cue");
    }

    #[test]
    fn reports_missing_lyrics_as_unavailable() {
        let error = parse_lyrics(br#"{"code": 404, "lrc": {"lyric": ""}}"#)
            .err()
            .unwrap_or_else(|| unreachable!());
        assert_eq!(error.code, "SOURCE_LYRICS_UNAVAILABLE");
        assert!(!error.retryable);
    }

    #[test]
    fn maps_playlist_search_entries() {
        let page = parse_playlist_search(PLAYLIST_SEARCH.as_bytes(), 1, 20)
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(page.total, Some(2));
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "900");
        assert_eq!(page.items[0].creator.as_deref(), Some("DJ Test"));
        assert_eq!(page.items[0].track_count, Some(42));
        assert!(matches!(
            page.items[0].content_type,
            CollectionType::Playlist
        ));
    }

    #[test]
    fn maps_album_search_entries() {
        let page =
            parse_album_search(ALBUM_SEARCH.as_bytes(), 2, 10).unwrap_or_else(|_| unreachable!());
        assert_eq!(page.page, 2);
        assert_eq!(page.items[0].id, "700");
        assert_eq!(page.items[0].creator.as_deref(), Some("Artist A"));
        assert!(matches!(page.items[0].content_type, CollectionType::Album));
    }

    #[test]
    fn returns_empty_page_for_zero_result_search() {
        let page = parse_playlist_search(br#"{"result": {"playlistCount": 0}}"#, 1, 20)
            .unwrap_or_else(|_| unreachable!());
        assert!(page.items.is_empty());
        assert_eq!(page.total, Some(0));
        let songs = parse_search(br#"{"result": {"songCount": 0, "songs": []}}"#, 1, 20)
            .unwrap_or_else(|_| unreachable!());
        assert!(songs.items.is_empty());
    }

    #[test]
    fn maps_playlist_detail_with_every_track() {
        let detail =
            parse_playlist_detail(PLAYLIST_DETAIL.as_bytes()).unwrap_or_else(|_| unreachable!());
        assert_eq!(detail.collection.name, "Focus Mix");
        assert_eq!(detail.songs.len(), 2);
        assert_eq!(detail.songs[0].reference.site_id, "111");
        assert_eq!(detail.songs[0].duration_ms, Some(210_000));
        assert_eq!(detail.songs[1].artists[0].name, "Artist B");
        assert_eq!(
            detail.songs[0].cover_url.as_deref(),
            Some("https://example.test/a.jpg")
        );
    }

    #[test]
    fn maps_album_detail_tracks() {
        let detail = parse_album_detail(ALBUM_DETAIL.as_bytes()).unwrap_or_else(|_| unreachable!());
        assert_eq!(detail.collection.id, "700");
        assert!(matches!(
            detail.collection.content_type,
            CollectionType::Album
        ));
        assert_eq!(detail.songs.len(), 1);
        assert_eq!(detail.songs[0].title, "Track One");
    }

    #[test]
    fn maps_absent_collection_to_source_error() {
        let error = parse_playlist_detail(br#"{"code": 401}"#)
            .err()
            .unwrap_or_else(|| unreachable!());
        assert_eq!(error.code, "SOURCE_COLLECTION_UNAVAILABLE");
        assert!(error.message.contains("401"));
        assert!(!error.retryable);
    }

    #[test]
    fn maps_malformed_payload_to_response_invalid() {
        let error = parse_album_detail(b"<html>error</html>")
            .err()
            .unwrap_or_else(|| unreachable!());
        assert_eq!(error.code, "SOURCE_RESPONSE_INVALID");
        assert!(!error.retryable);
    }

    #[test]
    fn advertises_the_capabilities_it_implements() {
        let source = NeteaseSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let capabilities = source.capabilities();
        assert!(capabilities.playlist_search);
        assert!(capabilities.album_search);
        assert!(capabilities.lyrics);
        assert!(capabilities.qr_login);
        assert!(capabilities.qualities.contains(&"320000".to_owned()));
    }

    #[tokio::test]
    #[ignore = "requires live Netease service"]
    async fn fetches_live_playlist_detail() {
        let source = NeteaseSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
        let detail = source
            .playlist_detail("3778678")
            .await
            .unwrap_or_else(|error| panic!("live playlist detail failed: {error:?}"));
        assert!(!detail.songs.is_empty());
    }
}
