//! Metadata embedding and reading via `lofty`.
//!
//! Priority rules: only non-empty incoming values overwrite, everything else
//! is preserved, an existing cover survives when no new one is supplied.
//! Writes go through `lofty`, so tagging needs no external process and keeps
//! working with ffmpeg absent, which is the normal case on a fresh machine.
//!
//! `wma` is in `domain::audio::TAGGABLE`, but lofty cannot write ASF. Rather
//! than introduce a subprocess dependency for one rare container, a wma
//! request reports `Unsupported`, which callers surface as a warning while
//! keeping the downloaded audio.

use std::path::Path;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{FileType, TaggedFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, AudioFile, ItemKey, TagExt};
use lofty::probe::Probe;
use lofty::tag::{Tag, TagType};

use crate::domain::audio::is_taggable;

#[derive(Debug, thiserror::Error)]
pub enum TaggingError {
    #[error("读写音频文件失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("容器不支持写入元数据：{0}")]
    Unsupported(String),
    #[error("解析音频标签失败：{0}")]
    Read(String),
    #[error("写入音频标签失败：{0}")]
    Write(String),
}

/// Metadata to embed. Every field is optional; empty values never overwrite.
#[derive(Debug, Clone, Default)]
pub struct MetadataPatch {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Plain or LRC lyric text, embedded as unsynchronised lyrics.
    pub lyrics: Option<String>,
    pub cover: Option<CoverArt>,
}

#[derive(Debug, Clone)]
pub struct CoverArt {
    pub data: Vec<u8>,
    /// Source Content-Type; normalised before writing.
    pub mime: Option<String>,
}

impl MetadataPatch {
    /// True when there is nothing to write.
    ///
    /// The audio is returned untouched in this case; rewriting a file
    /// to change nothing risks corrupting it for no gain.
    pub fn is_empty(&self) -> bool {
        !has_text(&self.title)
            && !has_text(&self.artist)
            && !has_text(&self.album)
            && !has_text(&self.lyrics)
            && self
                .cover
                .as_ref()
                .is_none_or(|cover| cover.data.is_empty())
    }
}

fn has_text(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|text| !text.trim().is_empty())
}

fn trimmed(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Outcome of an embed attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedOutcome {
    /// Tags were written.
    Written,
    /// Nothing to do: container not taggable, or no incoming values.
    Skipped(&'static str),
}

/// Tags read back from a file, for the local-library scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub year: Option<u32>,
    pub duration_ms: Option<u64>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub has_cover: bool,
}

/// Maps a Content-Type to a picture MIME type.
///
/// Recognise png/webp/gif, otherwise assume jpeg, because platform cover
/// endpoints frequently answer with a generic or missing Content-Type while
/// serving JPEG bytes.
fn normalize_cover_mime(mime: Option<&str>, data: &[u8]) -> MimeType {
    if let Some(detected) = detect_image_mime(data) {
        return detected;
    }
    let lowered = mime.unwrap_or_default().to_ascii_lowercase();
    if lowered.contains("png") {
        MimeType::Png
    } else if lowered.contains("webp") {
        // lofty has no WebP variant; the raw string keeps the type honest.
        MimeType::Unknown("image/webp".to_owned())
    } else if lowered.contains("gif") {
        MimeType::Gif
    } else {
        MimeType::Jpeg
    }
}

/// Magic-number sniffing; the bytes are authoritative over a served header.
fn detect_image_mime(data: &[u8]) -> Option<MimeType> {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some(MimeType::Png);
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(MimeType::Jpeg);
    }
    if data.starts_with(b"GIF8") {
        return Some(MimeType::Gif);
    }
    if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return Some(MimeType::Unknown("image/webp".to_owned()));
    }
    None
}

/// Extension in the form `domain::audio` reports, lowercased, no dot.
fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
}

/// Tag format to write for a container.
fn tag_type_for(file_type: FileType) -> Option<TagType> {
    match file_type {
        FileType::Mpeg | FileType::Aac | FileType::Wav | FileType::Aiff => Some(TagType::Id3v2),
        FileType::Flac | FileType::Opus | FileType::Vorbis | FileType::Speex => {
            Some(TagType::VorbisComments)
        }
        FileType::Mp4 => Some(TagType::Mp4Ilst),
        _ => None,
    }
}

fn open_tagged_file(path: &Path) -> Result<TaggedFile, TaggingError> {
    Probe::open(path)
        .map_err(|error| TaggingError::Read(error.to_string()))?
        .options(ParseOptions::new().read_properties(true))
        .read()
        .map_err(|error| TaggingError::Read(error.to_string()))
}

/// Lyric key for a tag format.
///
/// ID3v2 has no plain `Lyrics` item; lyrics are written as unsynchronised
/// lyrics (`USLT`), and lofty exposes that frame under a distinct key.
fn lyric_key(tag_type: TagType) -> ItemKey {
    match tag_type {
        TagType::Id3v2 => ItemKey::UnsyncLyrics,
        _ => ItemKey::Lyrics,
    }
}

/// Base tag to mutate: the existing tag of the target format when present,
/// otherwise a new tag seeded from whatever tag the file already carries.
///
/// Seeding matters for a flac holding only a read-only ID3v2 block: without it
/// the previous title/artist/album/cover would silently vanish.
fn base_tag(file: &TaggedFile, target: TagType) -> Tag {
    if let Some(existing) = file.tag(target) {
        return existing.clone();
    }
    let mut tag = Tag::new(target);
    if let Some(source) = file.first_tag() {
        for (key, value) in [
            (ItemKey::TrackTitle, source.title()),
            (ItemKey::TrackArtist, source.artist()),
            (ItemKey::AlbumTitle, source.album()),
        ] {
            if let Some(text) = value
                .map(|text| text.trim().to_owned())
                .filter(|t| !t.is_empty())
            {
                tag.insert_text(key, text);
            }
        }
        if let Some(text) = existing_lyrics(source) {
            tag.insert_text(lyric_key(target), text);
        }
        for picture in source.pictures() {
            tag.push_picture(picture.clone());
        }
    }
    tag
}

/// Reads embedded lyrics under either key.
fn existing_lyrics(tag: &Tag) -> Option<String> {
    [ItemKey::UnsyncLyrics, ItemKey::Lyrics]
        .into_iter()
        .filter_map(|key| tag.get_string(key))
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Embeds `patch` into the file at `path`, preserving whatever it does not set.
///
/// Returns `Skipped` for containers outside `domain::audio::TAGGABLE` and for
/// an all-empty patch. Errors leave the file untouched — lofty writes tags in
/// place and validates before committing, so a rejected write does not
/// truncate the audio.
pub fn embed_metadata(path: &Path, patch: &MetadataPatch) -> Result<EmbedOutcome, TaggingError> {
    let extension =
        extension_of(path).ok_or_else(|| TaggingError::Unsupported("文件缺少扩展名".to_owned()))?;
    if !is_taggable(&extension) {
        return Ok(EmbedOutcome::Skipped("容器不支持内嵌元数据"));
    }
    if patch.is_empty() {
        return Ok(EmbedOutcome::Skipped("没有可写入的元数据"));
    }

    let file = open_tagged_file(path)?;
    let target = tag_type_for(file.file_type()).ok_or_else(|| {
        // wma lands here: writing ASF tags would need ffmpeg, which this build
        // deliberately does not require.
        TaggingError::Unsupported(format!("{extension} 容器无法写入标签"))
    })?;

    let mut tag = base_tag(&file, target);
    if let Some(title) = trimmed(&patch.title) {
        tag.set_title(title);
    }
    if let Some(artist) = trimmed(&patch.artist) {
        tag.set_artist(artist);
    }
    if let Some(album) = trimmed(&patch.album) {
        tag.set_album(album);
    }
    if let Some(lyrics) = trimmed(&patch.lyrics) {
        tag.insert_text(lyric_key(target), lyrics);
    }
    if let Some(cover) = patch.cover.as_ref().filter(|cover| !cover.data.is_empty()) {
        let mime = normalize_cover_mime(cover.mime.as_deref(), &cover.data);
        // Replace only the front cover, so a booklet or artist image the file
        // already carries survives.
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(
            Picture::unchecked(cover.data.clone())
                .pic_type(PictureType::CoverFront)
                .mime_type(mime)
                .build(),
        );
    }

    tag.save_to_path(path, WriteOptions::new())
        .map_err(|error| TaggingError::Write(error.to_string()))?;
    Ok(EmbedOutcome::Written)
}

/// Reads tags and audio properties from a file.
///
/// Synchronous and process-free, so a blocking library scan can call it
/// directly and it works with ffmpeg absent.
pub fn read_tags(path: &Path) -> Result<TrackTags, TaggingError> {
    let file = open_tagged_file(path)?;
    let properties = file.properties();
    let duration = properties.duration().as_millis();
    let mut tags = TrackTags {
        duration_ms: (duration > 0).then_some(duration as u64),
        bitrate_kbps: properties
            .audio_bitrate()
            .or_else(|| properties.overall_bitrate())
            .filter(|value| *value > 0),
        sample_rate: properties.sample_rate().filter(|value| *value > 0),
        has_cover: file.tags().iter().any(|tag| !tag.pictures().is_empty()),
        ..TrackTags::default()
    };

    let source = file.primary_tag().or_else(|| file.first_tag());
    if let Some(tag) = source {
        tags.title = non_empty(tag.title());
        tags.artist = non_empty(tag.artist());
        tags.album = non_empty(tag.album());
        tags.album_artist = tag
            .get_string(ItemKey::AlbumArtist)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
        tags.track_number = tag.track();
        // No `year()` accessor exists; the value is a plain text item, and a
        // full date string ("2019-04-01") still yields the leading year.
        tags.year = tag
            .get_string(ItemKey::RecordingDate)
            .or_else(|| tag.get_string(ItemKey::Year))
            .and_then(|text| text.trim().get(..4).and_then(|y| y.parse::<u32>().ok()))
            .filter(|year| *year > 0);
    }
    Ok(tags)
}

fn non_empty(value: Option<std::borrow::Cow<'_, str>>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        embed_metadata, normalize_cover_mime, read_tags, CoverArt, EmbedOutcome, MetadataPatch,
    };
    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::prelude::{Accessor, TagExt, TaggedFileExt};
    use lofty::tag::{Tag, TagType};
    use std::path::{Path, PathBuf};

    /// Smallest decodable JPEG-ish payload: only the signature is inspected.
    fn jpeg_bytes(marker: u8) -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xE0];
        data.extend_from_slice(&[marker; 32]);
        data.extend_from_slice(&[0xFF, 0xD9]);
        data
    }

    /// Writes a minimal but real mp3: two silent MPEG frames plus an ID3v2 tag.
    ///
    /// Building the fixture in-test keeps the suite free of binary assets and
    /// proves the round-trip against bytes lofty itself accepts. Two frames is
    /// the floor, not padding: lofty validates a frame by checking that the
    /// *next* header is consistent, so a single-frame file fails to parse.
    fn make_mp3(dir: &Path) -> PathBuf {
        let path = dir.join("fixture.mp3");
        // MPEG-1 Layer III, 128 kbps, 44.1 kHz, no padding: 417-byte frames.
        let mut frame = vec![0xFF, 0xFB, 0x90, 0x00];
        frame.resize(417, 0);
        let mut audio = frame.clone();
        audio.extend_from_slice(&frame);
        std::fs::write(&path, &audio).expect("write mp3 frames");
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title("seed".to_owned());
        tag.save_to_path(&path, WriteOptions::new())
            .expect("seed id3v2");
        path
    }

    /// Writes a minimal FLAC stream: signature plus a STREAMINFO block.
    fn make_flac(dir: &Path) -> PathBuf {
        let path = dir.join("fixture.flac");
        let mut data = Vec::from(*b"fLaC");
        // Last-metadata-block flag set, type 0 (STREAMINFO), length 34.
        data.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
        let mut info = [0u8; 34];
        // min/max block size 4096.
        info[0..2].copy_from_slice(&4096u16.to_be_bytes());
        info[2..4].copy_from_slice(&4096u16.to_be_bytes());
        // 44100 Hz (20 bits), 2 channels, 16 bits per sample, 0 total samples.
        info[10] = 0x0A;
        info[11] = 0xC4;
        info[12] = 0x42;
        info[13] = 0xF0;
        data.extend_from_slice(&info);
        std::fs::write(&path, &data).expect("write flac");
        path
    }

    #[test]
    fn mp3_round_trips_title_artist_album() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = make_mp3(dir.path());

        let outcome = embed_metadata(
            &path,
            &MetadataPatch {
                title: Some("曲名".to_owned()),
                artist: Some("歌手".to_owned()),
                album: Some("专辑".to_owned()),
                ..MetadataPatch::default()
            },
        )
        .expect("embed mp3");
        assert_eq!(outcome, EmbedOutcome::Written);

        let tags = read_tags(&path).expect("read mp3 tags");
        assert_eq!(tags.title.as_deref(), Some("曲名"));
        assert_eq!(tags.artist.as_deref(), Some("歌手"));
        assert_eq!(tags.album.as_deref(), Some("专辑"));
    }

    #[test]
    fn flac_round_trips_title_artist_album() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = make_flac(dir.path());

        embed_metadata(
            &path,
            &MetadataPatch {
                title: Some("flac 曲名".to_owned()),
                artist: Some("flac 歌手".to_owned()),
                album: Some("flac 专辑".to_owned()),
                ..MetadataPatch::default()
            },
        )
        .expect("embed flac");

        let tags = read_tags(&path).expect("read flac tags");
        assert_eq!(tags.title.as_deref(), Some("flac 曲名"));
        assert_eq!(tags.artist.as_deref(), Some("flac 歌手"));
        assert_eq!(tags.album.as_deref(), Some("flac 专辑"));
    }

    #[test]
    fn empty_incoming_values_preserve_existing_tags() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = make_mp3(dir.path());
        embed_metadata(
            &path,
            &MetadataPatch {
                title: Some("原标题".to_owned()),
                artist: Some("原歌手".to_owned()),
                ..MetadataPatch::default()
            },
        )
        .expect("first embed");

        // Only the album is supplied; blank and absent fields must not clear.
        embed_metadata(
            &path,
            &MetadataPatch {
                title: Some("   ".to_owned()),
                album: Some("新专辑".to_owned()),
                ..MetadataPatch::default()
            },
        )
        .expect("second embed");

        let tags = read_tags(&path).expect("read tags");
        assert_eq!(tags.title.as_deref(), Some("原标题"));
        assert_eq!(tags.artist.as_deref(), Some("原歌手"));
        assert_eq!(tags.album.as_deref(), Some("新专辑"));
    }

    #[test]
    fn existing_cover_survives_when_no_cover_supplied() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = make_mp3(dir.path());
        let original = jpeg_bytes(0x11);

        let mut seeded = Tag::new(TagType::Id3v2);
        seeded.set_title("seed".to_owned());
        seeded.push_picture(
            Picture::unchecked(original.clone())
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Jpeg)
                .build(),
        );
        seeded
            .save_to_path(&path, WriteOptions::new())
            .expect("seed cover");

        embed_metadata(
            &path,
            &MetadataPatch {
                title: Some("新标题".to_owned()),
                ..MetadataPatch::default()
            },
        )
        .expect("embed without cover");

        let tags = read_tags(&path).expect("read tags");
        assert_eq!(tags.title.as_deref(), Some("新标题"));
        assert!(tags.has_cover, "existing cover must be preserved");
    }

    #[test]
    fn supplied_cover_replaces_front_cover() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = make_mp3(dir.path());
        let replacement = jpeg_bytes(0x22);

        embed_metadata(
            &path,
            &MetadataPatch {
                cover: Some(CoverArt {
                    data: replacement.clone(),
                    mime: Some("image/jpeg".to_owned()),
                }),
                ..MetadataPatch::default()
            },
        )
        .expect("embed cover");

        let file = super::open_tagged_file(&path).expect("reopen");
        let picture = file
            .tags()
            .iter()
            .flat_map(|tag| tag.pictures())
            .find(|picture| picture.pic_type() == PictureType::CoverFront)
            .expect("front cover present");
        assert_eq!(picture.data(), replacement.as_slice());
    }

    #[test]
    fn non_taggable_container_is_skipped() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("audio.ogg");
        std::fs::write(&path, b"OggS placeholder").expect("write ogg");

        let outcome = embed_metadata(
            &path,
            &MetadataPatch {
                title: Some("x".to_owned()),
                ..MetadataPatch::default()
            },
        )
        .expect("skip non-taggable");
        assert!(matches!(outcome, EmbedOutcome::Skipped(_)));
    }

    #[test]
    fn all_empty_patch_is_skipped_without_touching_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = make_mp3(dir.path());
        let before = std::fs::read(&path).expect("read before");

        let outcome = embed_metadata(
            &path,
            &MetadataPatch {
                title: Some("  ".to_owned()),
                artist: None,
                album: Some(String::new()),
                lyrics: None,
                cover: Some(CoverArt {
                    data: Vec::new(),
                    mime: None,
                }),
            },
        )
        .expect("skip empty");
        assert!(matches!(outcome, EmbedOutcome::Skipped(_)));
        assert_eq!(
            std::fs::read(&path).expect("read after"),
            before,
            "an empty patch must not rewrite the file"
        );
    }

    #[test]
    fn lyrics_embed_and_read_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = make_mp3(dir.path());
        embed_metadata(
            &path,
            &MetadataPatch {
                lyrics: Some("[00:01.00]alpha".to_owned()),
                ..MetadataPatch::default()
            },
        )
        .expect("embed lyrics");

        let file = super::open_tagged_file(&path).expect("reopen");
        let stored = file
            .tags()
            .iter()
            .find_map(super::existing_lyrics)
            .expect("lyrics stored");
        assert_eq!(stored, "[00:01.00]alpha");
    }

    #[test]
    fn cover_mime_prefers_signature_over_header() {
        // A served Content-Type of PNG over JPEG bytes must not win.
        assert_eq!(
            normalize_cover_mime(Some("image/png"), &jpeg_bytes(0x01)),
            MimeType::Jpeg
        );
        assert_eq!(normalize_cover_mime(Some("image/png"), &[]), MimeType::Png);
        assert_eq!(normalize_cover_mime(None, &[]), MimeType::Jpeg);
    }

    #[test]
    fn missing_file_reports_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let result = read_tags(&dir.path().join("absent.mp3"));
        assert!(result.is_err());
    }
}
