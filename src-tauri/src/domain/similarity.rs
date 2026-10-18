//! Ranking and matching primitives.
//!
//! Shared by auto source switching and search ranking so both score
//! candidates identically.

/// Lowercases and keeps only letters, numbers and CJK ideographs.
pub fn normalize_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

/// Rune-level Levenshtein distance with two rolling rows.
pub fn levenshtein_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0_usize; right.len() + 1];
    for (i, left_char) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_char) in right.iter().enumerate() {
            let cost = usize::from(left_char != right_char);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// `1 - distance / max_rune_len`, clamped to zero.
pub fn similarity_score(left: &str, right: &str) -> f64 {
    let left = normalize_text(left);
    let right = normalize_text(right);
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let max_len = left.chars().count().max(right.chars().count());
    if max_len == 0 {
        return 0.0;
    }
    let distance = levenshtein_distance(&left, &right);
    if distance >= max_len {
        0.0
    } else {
        1.0 - distance as f64 / max_len as f64
    }
}

/// Weighted title/artist similarity: `title * 0.7 + artist * 0.3`.
///
/// Degrades to title-only when either artist is missing; returns 0 when either
/// title is missing.
pub fn song_similarity(
    title: &str,
    artist: &str,
    candidate_title: &str,
    candidate_artist: &str,
) -> f64 {
    if title.trim().is_empty() || candidate_title.trim().is_empty() {
        return 0.0;
    }
    let title_score = similarity_score(title, candidate_title);
    if artist.trim().is_empty() || candidate_artist.trim().is_empty() {
        return title_score;
    }
    title_score * 0.7 + similarity_score(artist, candidate_artist) * 0.3
}

/// Duration proximity in milliseconds: within 10s, or within 15% of the
/// reference, whichever is larger. Unknown durations always match.
pub fn duration_close(left_ms: u64, right_ms: u64) -> bool {
    if left_ms == 0 || right_ms == 0 {
        return true;
    }
    let difference = left_ms.abs_diff(right_ms) as f64;
    if difference <= 10_000.0 {
        return true;
    }
    difference <= (left_ms as f64 * 0.15).max(10_000.0)
}

/// Human readable byte size; `-` for unknown.
pub fn format_size(bytes: Option<u64>) -> String {
    match bytes {
        None | Some(0) => "-".to_owned(),
        Some(value) => format!("{:.1} MB", value as f64 / (1024.0 * 1024.0)),
    }
}

/// Dedup key for downloads: `"{artist} - {title}"` with control characters
/// stripped and `Unknown` filled in for blanks.
pub fn song_key(artist: &str, title: &str) -> String {
    fn clean(value: &str, fallback: &str) -> String {
        let stripped: String = value.chars().filter(|ch| !ch.is_control()).collect();
        let trimmed = stripped.trim();
        if trimmed.is_empty() {
            fallback.to_owned()
        } else {
            trimmed.to_owned()
        }
    }
    format!("{} - {}", clean(artist, "Unknown"), clean(title, "Unknown"))
}

#[cfg(test)]
mod tests {
    use super::{
        duration_close, format_size, levenshtein_distance, normalize_text, similarity_score,
        song_key, song_similarity,
    };

    #[test]
    fn normalize_strips_punctuation_and_case() {
        assert_eq!(normalize_text("Hello, World! (Live)"), "helloworldlive");
        assert_eq!(normalize_text("七里香 - Remix"), "七里香remix");
    }

    #[test]
    fn levenshtein_counts_runes_not_bytes() {
        assert_eq!(levenshtein_distance("七里香", "七里响"), 1);
        assert_eq!(levenshtein_distance("", "abc"), 3);
    }

    #[test]
    fn identical_titles_score_one_and_disjoint_score_zero() {
        assert_eq!(similarity_score("七里香", "七里香"), 1.0);
        assert_eq!(similarity_score("abc", "xyz"), 0.0);
    }

    #[test]
    fn song_similarity_weights_title_over_artist() {
        let both = song_similarity("七里香", "周杰伦", "七里香", "周杰伦");
        let wrong_artist = song_similarity("七里香", "周杰伦", "七里香", "林俊杰");
        assert_eq!(both, 1.0);
        assert!((0.7..1.0).contains(&wrong_artist));
    }

    #[test]
    fn missing_artist_degrades_to_title_only() {
        assert_eq!(song_similarity("七里香", "", "七里香", "周杰伦"), 1.0);
    }

    #[test]
    fn missing_title_scores_zero() {
        assert_eq!(song_similarity("", "周杰伦", "七里香", "周杰伦"), 0.0);
    }

    #[test]
    fn duration_tolerance_uses_ten_seconds_then_percentage() {
        assert!(duration_close(0, 500_000));
        assert!(duration_close(240_000, 249_000));
        assert!(duration_close(240_000, 275_000));
        assert!(!duration_close(240_000, 300_000));
    }

    #[test]
    fn format_size_marks_unknown() {
        assert_eq!(format_size(None), "-");
        assert_eq!(format_size(Some(0)), "-");
        assert_eq!(format_size(Some(5 * 1024 * 1024)), "5.0 MB");
    }

    #[test]
    fn song_key_fills_unknown_and_strips_control_chars() {
        assert_eq!(song_key("周杰伦", "七里香"), "周杰伦 - 七里香");
        assert_eq!(song_key("  ", "\u{7}七里香"), "Unknown - 七里香");
    }
}
