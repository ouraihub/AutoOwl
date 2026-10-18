//! LRC parsing shared by source adapters and the lyrics use case.
//!
//! Lives in `domain` because both `infrastructure::sources` (platform payloads
//! arrive as LRC text) and `application::lyrics` (file export, dual-language
//! view) need it, and neither may depend on the other.

use std::borrow::Cow;

use super::media::LyricLine;

/// Parses LRC text into time-ordered lines.
///
/// Tolerates the shapes real platforms emit: `[mm:ss]`, `[mm:ss.xx]`,
/// `[mm:ss.xxx]`, several timestamps sharing one text, `[offset:±ms]` shifts,
/// metadata tags, and lines delivered out of order. Text with no timestamp at
/// all is kept as an untimed line so plain-text lyrics still render.
pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut offset_ms = 0i64;
    let mut timed: Vec<(u64, String)> = Vec::new();
    let mut untimed: Vec<String> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let (stamps, body) = split_timestamps(line);

        // `[offset:-500]` shifts every timing; it carries no lyric text.
        if stamps.is_empty() {
            if let Some(value) = metadata_offset(line) {
                offset_ms = value;
                continue;
            }
            if is_metadata(line) {
                continue;
            }
            untimed.push(line.to_owned());
            continue;
        }

        // Karaoke LRC carries per-word timings inline; collapse them away so
        // the rendered line is plain text.
        let body = strip_inline_timestamps(body);
        let body = body.trim();
        for stamp in stamps {
            timed.push((apply_offset(stamp, offset_ms), body.to_owned()));
        }
    }

    if timed.is_empty() {
        return untimed
            .into_iter()
            .map(|text| LyricLine {
                start_ms: None,
                end_ms: None,
                text,
            })
            .collect();
    }

    timed.sort_by_key(|(start, _)| *start);

    // A line ends where the next one begins; the last line stays open because
    // LRC carries no duration for it.
    let mut lines: Vec<LyricLine> = Vec::with_capacity(timed.len());
    for index in 0..timed.len() {
        let (start, ref text) = timed[index];
        let end = timed
            .get(index + 1)
            .map(|(next_start, _)| *next_start)
            .filter(|next_start| *next_start > start);
        lines.push(LyricLine {
            start_ms: Some(start),
            end_ms: end,
            text: text.clone(),
        });
    }
    lines
}

/// Merges a translation track into `original`, aligning on identical timings.
///
/// Unmatched translation lines are inserted in time order rather than dropped,
/// so a translation that carries extra lines stays visible.
pub fn merge_translation(original: Vec<LyricLine>, translated: &str) -> Vec<LyricLine> {
    let translation = parse_lrc(translated);
    if translation.is_empty() {
        return original;
    }

    let mut merged = original;
    for line in translation {
        if line.text.is_empty() {
            continue;
        }
        match merged
            .iter_mut()
            .find(|existing| existing.start_ms.is_some() && existing.start_ms == line.start_ms)
        {
            Some(existing) => {
                if !existing.text.is_empty() {
                    existing.text.push('\n');
                }
                existing.text.push_str(&line.text);
            }
            None => merged.push(line),
        }
    }

    merged.sort_by_key(|line| line.start_ms.unwrap_or(u64::MAX));
    merged
}

/// Renders lines back to LRC text for on-disk `.lrc` sidecars.
pub fn to_lrc(lines: &[LyricLine]) -> String {
    let mut out = String::new();
    for line in lines {
        if let Some(start) = line.start_ms {
            // Each physical text line needs its own timestamp, otherwise a
            // merged dual-language line would lose its timing on reparse.
            for piece in line.text.split('\n') {
                out.push_str(&format_timestamp(start));
                out.push_str(piece);
                out.push('\n');
            }
        } else {
            out.push_str(&line.text);
            out.push('\n');
        }
    }
    out
}

fn format_timestamp(total_ms: u64) -> String {
    let minutes = total_ms / 60_000;
    let seconds = (total_ms % 60_000) / 1_000;
    let hundredths = (total_ms % 1_000) / 10;
    format!("[{minutes:02}:{seconds:02}.{hundredths:02}]")
}

fn apply_offset(start_ms: u64, offset_ms: i64) -> u64 {
    // A negative offset larger than the timestamp clamps to zero instead of
    // wrapping around.
    let shifted = start_ms as i64 + offset_ms;
    shifted.max(0) as u64
}

/// Peels leading `[..]` groups off a line, returning parsed timings and the
/// remaining text.
fn split_timestamps(line: &str) -> (Vec<u64>, &str) {
    let mut stamps = Vec::new();
    let mut rest = line;

    while rest.starts_with('[') {
        let Some(close) = rest.find(']') else { break };
        let Some(stamp) = parse_timestamp(&rest[1..close]) else {
            break;
        };
        stamps.push(stamp);
        rest = &rest[close + 1..];
    }

    (stamps, rest)
}

/// Drops karaoke per-word timings (`[00:12.30]`) embedded inside lyric text.
///
/// Only groups that actually parse as a timestamp are removed, so annotations
/// a singer expects to read — `[间奏]`, `[Chorus]`, `[笑]` — survive verbatim.
/// Borrows unchanged when the line carries no inline timing, which is the
/// overwhelmingly common case.
fn strip_inline_timestamps(text: &str) -> Cow<'_, str> {
    let Some((first_start, first_end)) = find_inline_timestamp(text) else {
        return Cow::Borrowed(text);
    };

    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..first_start]);
    let mut rest = &text[first_end..];
    while let Some((start, end)) = find_inline_timestamp(rest) {
        out.push_str(&rest[..start]);
        rest = &rest[end..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Locates the next `[..]` group that parses as a timestamp, as a byte range.
fn find_inline_timestamp(text: &str) -> Option<(usize, usize)> {
    let mut search = 0usize;
    while let Some(offset) = text[search..].find('[') {
        let open = search + offset;
        // Without a closing bracket no later `[` can close either, so stop.
        let close = open + 1 + text[open + 1..].find(']')?;
        if parse_timestamp(&text[open + 1..close]).is_some() {
            return Some((open, close + 1));
        }
        search = open + 1;
    }
    None
}

/// Parses `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` into milliseconds.
fn parse_timestamp(body: &str) -> Option<u64> {
    let (minutes, tail) = body.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;

    let (seconds, fraction) = match tail.split_once(['.', ':']) {
        Some((seconds, fraction)) => (seconds, Some(fraction)),
        None => (tail, None),
    };
    let seconds: u64 = seconds.trim().parse().ok()?;
    if seconds >= 60 {
        return None;
    }

    let fraction_ms = match fraction {
        Some(digits) => {
            let digits = digits.trim();
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            // Two digits are hundredths, three are already milliseconds.
            let value: u64 = digits.parse().ok()?;
            match digits.len() {
                1 => value * 100,
                2 => value * 10,
                3 => value,
                _ => value / 10u64.pow(digits.len() as u32 - 3),
            }
        }
        None => 0,
    };

    Some(minutes * 60_000 + seconds * 1_000 + fraction_ms)
}

fn metadata_offset(line: &str) -> Option<i64> {
    let body = line.strip_prefix('[')?.strip_suffix(']')?;
    let value = body.strip_prefix("offset:")?;
    value.trim().parse().ok()
}

/// Recognises `[ti:]`, `[ar:]`, `[by:]` and friends: a bracketed tag whose key
/// is alphabetic, which no timestamp ever is.
fn is_metadata(line: &str) -> bool {
    let Some(body) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) else {
        return false;
    };
    match body.split_once(':') {
        Some((key, _)) => !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_alphabetic()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_timestamp_precisions() {
        let lines = parse_lrc("[00:01]a\n[00:02.5]b\n[00:03.25]c\n[00:04.125]d");
        let starts: Vec<Option<u64>> = lines.iter().map(|line| line.start_ms).collect();
        assert_eq!(
            starts,
            vec![Some(1_000), Some(2_500), Some(3_250), Some(4_125)]
        );
    }

    #[test]
    fn sorts_out_of_order_lines_and_fills_end_from_next_start() {
        let lines = parse_lrc("[00:10.00]second\n[00:05.00]first");
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[0].start_ms, Some(5_000));
        assert_eq!(lines[0].end_ms, Some(10_000));
        // The final line has no successor, so it stays open-ended.
        assert_eq!(lines[1].end_ms, None);
    }

    #[test]
    fn expands_repeated_timestamps_sharing_one_text() {
        let lines = parse_lrc("[00:01.00][00:31.00]chorus");
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.text == "chorus"));
        assert_eq!(lines[1].start_ms, Some(31_000));
    }

    #[test]
    fn applies_offset_and_clamps_below_zero() {
        let lines = parse_lrc("[offset:-500]\n[00:00.20]early\n[00:10.00]late");
        assert_eq!(lines[0].start_ms, Some(0));
        assert_eq!(lines[1].start_ms, Some(9_500));
    }

    #[test]
    fn skips_metadata_tags_but_keeps_untimed_text() {
        let tagged = parse_lrc("[ti:Song]\n[ar:Artist]\n[00:01.00]line");
        assert_eq!(tagged.len(), 1);
        assert_eq!(tagged[0].text, "line");

        // Plain-text lyrics carry no timings at all and must still render.
        let plain = parse_lrc("first line\nsecond line");
        assert_eq!(plain.len(), 2);
        assert!(plain.iter().all(|line| line.start_ms.is_none()));
    }

    #[test]
    fn rejects_invalid_timestamps_as_text() {
        // 75 seconds and a non-numeric fraction are not timestamps.
        let lines = parse_lrc("[00:75.00]bad\n[aa:bb.cc]worse");
        assert!(lines.iter().all(|line| line.start_ms.is_none()));
    }

    #[test]
    fn merges_translation_on_matching_timings() {
        let original = parse_lrc("[00:01.00]hello\n[00:02.00]world");
        let merged = merge_translation(original, "[00:01.00]你好");
        assert_eq!(merged[0].text, "hello\n你好");
        assert_eq!(merged[1].text, "world");
    }

    #[test]
    fn keeps_translation_lines_without_a_match() {
        let original = parse_lrc("[00:02.00]world");
        let merged = merge_translation(original, "[00:01.00]你好");
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].text, "你好");
        assert_eq!(merged[1].text, "world");
    }

    #[test]
    fn round_trips_through_lrc_text() {
        let source = "[00:01.20]one\n[01:05.50]two";
        let reparsed = parse_lrc(&to_lrc(&parse_lrc(source)));
        let starts: Vec<Option<u64>> = reparsed.iter().map(|line| line.start_ms).collect();
        assert_eq!(starts, vec![Some(1_200), Some(65_500)]);
        assert_eq!(reparsed[1].text, "two");
    }

    #[test]
    fn merged_dual_language_line_keeps_both_timings_on_reparse() {
        let merged = merge_translation(parse_lrc("[00:01.00]hello"), "[00:01.00]你好");
        let reparsed = parse_lrc(&to_lrc(&merged));
        // Both halves share the timestamp, so neither loses its timing.
        assert_eq!(reparsed.len(), 2);
        assert!(reparsed.iter().all(|line| line.start_ms == Some(1_000)));
    }

    #[test]
    fn collapses_karaoke_word_timings_into_one_line() {
        let lines = parse_lrc("[00:12.00]我[00:12.30]爱[00:12.65]你");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].start_ms, Some(12_000));
        assert_eq!(lines[0].text, "我爱你");
    }

    #[test]
    fn keeps_bracketed_annotations_that_are_not_timestamps() {
        let lines = parse_lrc("[00:30.00]副歌 [间奏] 继续");
        assert_eq!(lines[0].text, "副歌 [间奏] 继续");
    }
}
