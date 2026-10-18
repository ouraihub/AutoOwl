//! Local library domain types.
//!
//! A local file is addressable as a regular song through
//! `SongRef{source:"local", scope:"song", site_id:<track id>}`, so the player
//! and collections treat scanned files exactly like remote results.

use serde::{Deserialize, Serialize};

use super::source::{AlbumRef, Artist, Song, SongRef};

/// Source id reserved for files indexed from disk.
pub const LOCAL_SOURCE: &str = "local";

/// Separators tag writers use to pack several performers into one field.
const ARTIST_SEPARATORS: [char; 4] = ['/', ';', '、', '&'];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalTrack {
    #[serde(rename = "ref")]
    pub reference: SongRef,
    pub id: String,
    pub path: String,
    pub title: String,
    pub artists: Vec<Artist>,
    pub album: Option<AlbumRef>,
    pub format: String,
    pub file_size: u64,
    pub modified_at: String,
    pub duration_ms: Option<u64>,
    pub cover_url: Option<String>,
}

impl LocalTrack {
    /// Projects the track onto the shared `Song` shape so it can be queued,
    /// recorded in playback history, or added to a collection.
    pub fn to_song(&self) -> Song {
        Song {
            reference: self.reference.clone(),
            title: self.title.clone(),
            artists: self.artists.clone(),
            album: self.album.clone(),
            duration_ms: self.duration_ms,
            cover_url: self.cover_url.clone(),
        }
    }

    /// Comma-joined performer names, or `None` when the file carries no artist.
    pub fn artist_text(&self) -> Option<String> {
        if self.artists.is_empty() {
            return None;
        }
        Some(
            self.artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

/// Technical details kept out of the hot columns and stored as JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalTrackMetadata {
    pub duration_ms: Option<u64>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub track_number: Option<u32>,
    pub year: Option<u32>,
    pub album_artist: Option<String>,
    pub has_cover: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
}

impl<T> Page<T> {
    /// True when further pages exist for the current window.
    pub fn has_more(&self) -> bool {
        let consumed = u64::from(self.page.saturating_sub(1))
            .saturating_mul(u64::from(self.page_size))
            .saturating_add(self.items.len() as u64);
        consumed < self.total
    }
}

/// Reference to a scanned file.
pub fn local_song_ref(track_id: &str) -> SongRef {
    SongRef {
        source: LOCAL_SOURCE.to_owned(),
        scope: "song".to_owned(),
        site_id: track_id.to_owned(),
    }
}

/// Splits a packed artist tag into individual performers.
pub fn split_artists(raw: &str) -> Vec<Artist> {
    raw.split(ARTIST_SEPARATORS)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| Artist {
            id: None,
            name: name.to_owned(),
        })
        .collect()
}

/// Identity used to spot the same recording arriving twice.
///
/// Duration is bucketed to whole seconds because re-encodes and tag rewrites
/// shift the millisecond value without changing the recording.
pub fn fingerprint(title: &str, artist: Option<&str>, duration_ms: Option<u64>) -> String {
    format!(
        "{}|{}|{}",
        title.trim().to_lowercase(),
        artist.unwrap_or_default().trim().to_lowercase(),
        duration_ms.unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::{fingerprint, local_song_ref, split_artists, LocalTrack, Page};

    fn track() -> LocalTrack {
        LocalTrack {
            reference: local_song_ref("abc"),
            id: "abc".into(),
            path: "D:/Music/a.mp3".into(),
            title: "Song".into(),
            artists: split_artists("Alice/Bob"),
            album: None,
            format: "mp3".into(),
            file_size: 12,
            modified_at: "1970-01-01T00:00:00Z".into(),
            duration_ms: Some(1000),
            cover_url: None,
        }
    }

    #[test]
    fn local_reference_is_addressable_as_a_song() {
        let song = track().to_song();
        assert_eq!(song.reference.source, "local");
        assert_eq!(song.reference.scope, "song");
        assert_eq!(song.reference.site_id, "abc");
        assert_eq!(song.artists.len(), 2);
    }

    #[test]
    fn packed_artist_tags_split_on_every_separator() {
        let names: Vec<_> = split_artists(" Alice / Bob;Carol、Dave & Eve ")
            .into_iter()
            .map(|artist| artist.name)
            .collect();
        assert_eq!(names, ["Alice", "Bob", "Carol", "Dave", "Eve"]);
        assert!(split_artists("  /  ").is_empty());
    }

    #[test]
    fn artist_text_is_none_without_performers() {
        let mut value = track();
        assert_eq!(value.artist_text().as_deref(), Some("Alice, Bob"));
        value.artists.clear();
        assert!(value.artist_text().is_none());
    }

    #[test]
    fn has_more_tracks_the_window_against_the_total() {
        let page = Page {
            items: vec![1, 2],
            page: 1,
            page_size: 2,
            total: 5,
        };
        assert!(page.has_more());
        let last = Page {
            items: vec![5],
            page: 3,
            page_size: 2,
            total: 5,
        };
        assert!(!last.has_more());
    }

    #[test]
    fn fingerprint_normalizes_case_and_padding() {
        assert_eq!(
            fingerprint(" Song ", Some(" Artist "), Some(1000)),
            "song|artist|1000"
        );
        assert_eq!(fingerprint("Song", None, None), "song||0");
    }
}
