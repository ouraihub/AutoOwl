//! Rules for mutating the local library.
//!
//! Kept separate from `local_music` because these are the checks that guard
//! writes: what an upload may be named, and when two indexed files are the
//! same recording.

use std::path::{Component, Path};

use crate::domain::local_music::{fingerprint, LocalTrack};

/// Rejects upload names that would escape the library directory.
pub fn validate_upload_name(name: &str) -> Result<(), String> {
    let path = Path::new(name);
    if name.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
    {
        return Err("上传文件名无效".to_owned());
    }
    Ok(())
}

/// Identity used to spot the same recording arriving twice.
pub fn duplicate_key(title: &str, artist: Option<&str>, duration_ms: Option<u64>) -> String {
    fingerprint(title, artist, duration_ms)
}

/// Groups indexed tracks that look like the same recording.
///
/// Only groups with more than one member are returned, because a group of one
/// is not a duplicate.
pub fn duplicate_groups(tracks: &[LocalTrack]) -> Vec<Vec<LocalTrack>> {
    let mut groups: Vec<(String, Vec<LocalTrack>)> = Vec::new();
    for track in tracks {
        let key = duplicate_key(
            &track.title,
            track.artist_text().as_deref(),
            track.duration_ms,
        );
        match groups.iter_mut().find(|(existing, _)| existing == &key) {
            Some((_, members)) => members.push(track.clone()),
            None => groups.push((key, vec![track.clone()])),
        }
    }
    groups
        .into_iter()
        .map(|(_, members)| members)
        .filter(|members| members.len() > 1)
        .collect()
}

/// True when `tracks` already contains a recording matching title and artist.
///
/// Duration is ignored: the caller is matching a search result against the
/// library, and the two sources rarely agree on duration to the millisecond.
pub fn library_contains(tracks: &[LocalTrack], title: &str, artist: Option<&str>) -> bool {
    let wanted_title = title.trim().to_lowercase();
    if wanted_title.is_empty() {
        return false;
    }
    let wanted_artist = artist.map(|value| value.trim().to_lowercase());
    tracks.iter().any(|track| {
        if track.title.trim().to_lowercase() != wanted_title {
            return false;
        }
        match &wanted_artist {
            // Without an artist to compare, a title match is enough.
            None => true,
            Some(expected) if expected.is_empty() => true,
            Some(expected) => track
                .artist_text()
                .map(|actual| actual.trim().to_lowercase().contains(expected.as_str()))
                .unwrap_or(false),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{duplicate_groups, duplicate_key, library_contains, validate_upload_name};
    use crate::domain::local_music::{local_song_ref, split_artists, LocalTrack};

    fn track(id: &str, title: &str, artist: &str, duration_ms: Option<u64>) -> LocalTrack {
        LocalTrack {
            reference: local_song_ref(id),
            id: id.to_owned(),
            path: format!("/music/{id}.mp3"),
            title: title.to_owned(),
            artists: split_artists(artist),
            album: None,
            format: "mp3".to_owned(),
            file_size: 1,
            modified_at: "1970-01-01T00:00:00Z".to_owned(),
            duration_ms,
            cover_url: None,
        }
    }

    #[test]
    fn rejects_traversal_and_absolute_uploads() {
        assert!(validate_upload_name("song.mp3").is_ok());
        assert!(validate_upload_name("../song.mp3").is_err());
        assert!(validate_upload_name("a/../../song.mp3").is_err());
        assert!(validate_upload_name("/etc/song.mp3").is_err());
        assert!(validate_upload_name("   ").is_err());
    }

    #[test]
    fn duplicate_key_ignores_case_and_padding() {
        assert_eq!(
            duplicate_key(" Song ", Some(" Artist "), Some(1000)),
            duplicate_key("song", Some("artist"), Some(1000))
        );
        assert_ne!(
            duplicate_key("Song", Some("Artist"), Some(1000)),
            duplicate_key("Song", Some("Artist"), Some(2000))
        );
    }

    #[test]
    fn only_real_duplicate_groups_are_reported() {
        let tracks = vec![
            track("1", "Song", "Artist", Some(1000)),
            track("2", "song", "artist", Some(1000)),
            track("3", "Other", "Artist", Some(1000)),
        ];
        let groups = duplicate_groups(&tracks);
        assert_eq!(groups.len(), 1);
        let ids: Vec<_> = groups[0].iter().map(|track| track.id.clone()).collect();
        assert_eq!(ids, ["1", "2"]);
    }

    #[test]
    fn library_match_needs_a_title_and_tolerates_extra_performers() {
        let tracks = vec![track("1", "Song", "Alice/Bob", Some(1000))];
        assert!(library_contains(&tracks, "song", Some("Alice")));
        assert!(library_contains(&tracks, "SONG", None));
        assert!(!library_contains(&tracks, "Song", Some("Carol")));
        assert!(!library_contains(&tracks, "  ", Some("Alice")));
        assert!(!library_contains(&tracks, "Missing", None));
    }
}
