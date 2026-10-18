//! Lyrics use case: dual-language view and `.lrc` sidecar export.
//!
//! LRC syntax lives in `domain::lyrics`; this module owns what the application
//! does with the parsed result.

use std::path::{Path, PathBuf};

use crate::domain::lyrics::{merge_translation, parse_lrc, to_lrc};
use crate::domain::media::Lyrics;

/// Builds the timeline the player renders.
///
/// The raw payloads stay on the struct so the UI can still show a single
/// language, while `lines` holds the merged dual-language timeline. A
/// translation is only merged when the source lyric carries timings; without them
/// there is nothing to align on and appending would interleave the two tracks
/// arbitrarily.
pub fn normalize_lyrics(
    original: Option<String>,
    translated: Option<String>,
    romanized: Option<String>,
) -> Lyrics {
    let mut lines = parse_lrc(original.as_deref().unwrap_or_default());
    let has_timings = lines.iter().any(|line| line.start_ms.is_some());

    if has_timings {
        if let Some(translation) = translated.as_deref().filter(|text| !text.trim().is_empty()) {
            lines = merge_translation(lines, translation);
        }
    }

    Lyrics {
        original,
        translated,
        romanized,
        lines,
    }
}

/// Renders the parsed timeline back to LRC text.
pub fn lrc_text(lyrics: &Lyrics) -> String {
    to_lrc(&lyrics.lines)
}

/// `.lrc` path for an audio file: same directory, same stem.
pub fn sidecar_path(audio_path: &Path) -> PathBuf {
    audio_path.with_extension("lrc")
}

/// Writes the timeline as an `.lrc` file beside the audio.
///
/// Skips empty lyrics rather than leaving a zero-byte sidecar that later looks
/// like a fetch succeeded with no content.
pub fn write_lrc_file(audio_path: &Path, lyrics: &Lyrics) -> std::io::Result<Option<PathBuf>> {
    let text = lrc_text(lyrics);
    if text.trim().is_empty() {
        return Ok(None);
    }
    let target = sidecar_path(audio_path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, text)?;
    Ok(Some(target))
}

#[cfg(test)]
mod tests {
    use super::{lrc_text, normalize_lyrics, sidecar_path, write_lrc_file};
    use std::path::Path;

    #[test]
    fn removes_empty_lyric_lines() {
        let lyrics = normalize_lyrics(Some("a\n\n b".into()), None, None);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[1].text, "b");
    }

    #[test]
    fn translation_merges_on_matching_timestamp_and_keeps_extras() {
        let lyrics = normalize_lyrics(
            Some("[00:01.00]alpha\n[00:02.00]beta".into()),
            Some("[00:01.00]first\n[00:09.00]orphan".into()),
            None,
        );
        assert_eq!(lyrics.lines[0].text, "alpha\nfirst");
        assert_eq!(lyrics.lines[1].text, "beta");
        let orphan = lyrics
            .lines
            .iter()
            .find(|line| line.start_ms == Some(9_000))
            .expect("unmatched translation line kept");
        assert_eq!(orphan.text, "orphan");
    }

    #[test]
    fn untimed_original_ignores_translation_merge() {
        let lyrics = normalize_lyrics(Some("plain text".into()), Some("[00:01.00]x".into()), None);
        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(lyrics.lines[0].text, "plain text");
    }

    #[test]
    fn round_trips_through_lrc_text() {
        let source = "[00:01.00]alpha\n[01:05.40]beta\n[02:00.00]gamma";
        let first = normalize_lyrics(Some(source.into()), None, None);
        let rendered = lrc_text(&first);
        let second = normalize_lyrics(Some(rendered), None, None);

        let original: Vec<_> = first
            .lines
            .iter()
            .map(|line| (line.start_ms, line.text.clone()))
            .collect();
        let reparsed: Vec<_> = second
            .lines
            .iter()
            .map(|line| (line.start_ms, line.text.clone()))
            .collect();
        assert_eq!(original, reparsed);
    }

    #[test]
    fn merged_dual_language_round_trips_both_languages() {
        let lyrics = normalize_lyrics(
            Some("[00:01.00]alpha".into()),
            Some("[00:01.00]first".into()),
            None,
        );
        let reparsed = normalize_lyrics(Some(lrc_text(&lyrics)), None, None);
        // Both physical lines keep the shared timestamp, so neither language
        // loses its timing on reparse.
        assert_eq!(reparsed.lines.len(), 2);
        assert!(reparsed
            .lines
            .iter()
            .all(|line| line.start_ms == Some(1_000)));
        let texts: Vec<_> = reparsed
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert!(texts.contains(&"alpha") && texts.contains(&"first"));
    }

    #[test]
    fn sidecar_replaces_audio_extension() {
        assert_eq!(
            sidecar_path(Path::new("music/song.flac")),
            Path::new("music/song.lrc")
        );
    }

    #[test]
    fn writes_sidecar_next_to_audio() {
        let dir = tempfile::tempdir().expect("temp dir");
        let audio = dir.path().join("track.mp3");
        let lyrics = normalize_lyrics(Some("[00:01.00]alpha".into()), None, None);

        let written = write_lrc_file(&audio, &lyrics)
            .expect("write sidecar")
            .expect("sidecar path");
        assert_eq!(written, dir.path().join("track.lrc"));
        let text = std::fs::read_to_string(&written).expect("read sidecar");
        assert_eq!(text.trim(), "[00:01.00]alpha");
    }

    #[test]
    fn empty_lyrics_write_no_sidecar() {
        let dir = tempfile::tempdir().expect("temp dir");
        let audio = dir.path().join("track.mp3");
        let lyrics = normalize_lyrics(Some("   ".into()), None, None);

        assert!(write_lrc_file(&audio, &lyrics)
            .expect("write skipped")
            .is_none());
        assert!(!dir.path().join("track.lrc").exists());
    }
}
