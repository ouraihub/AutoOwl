//! Audio container detection.
//!
//! Resolution order is magic number, then Content-Type, then `mp3` fallback.

const ASF_GUID: [u8; 16] = [
    0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
];

/// Containers that accept embedded metadata.
pub const TAGGABLE: [&str; 4] = ["mp3", "flac", "m4a", "wma"];

pub fn is_taggable(extension: &str) -> bool {
    TAGGABLE.contains(&extension)
}

/// Detects the container from leading bytes; `None` when unrecognised.
pub fn detect_by_signature(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 16 && data[..16] == ASF_GUID {
        return Some("wma");
    }
    if data.starts_with(b"fLaC") {
        return Some("flac");
    }
    if data.starts_with(b"ID3") {
        return Some("mp3");
    }
    // MPEG frame sync: 11 set bits.
    if data.len() >= 2 && data[0] == 0xFF && (data[1] & 0xE0) == 0xE0 {
        return Some("mp3");
    }
    if data.starts_with(b"OggS") {
        return Some("ogg");
    }
    if data.len() >= 8 && &data[4..8] == b"ftyp" {
        return Some("m4a");
    }
    None
}

/// Detects the container from a MIME type; `None` when unrecognised.
pub fn detect_by_content_type(content_type: &str) -> Option<&'static str> {
    let value = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    let table: [(&[&str], &str); 5] = [
        (&["audio/flac", "audio/x-flac"], "flac"),
        (
            &[
                "audio/x-ms-wma",
                "audio/wma",
                "video/x-ms-asf",
                "application/vnd.ms-asf",
            ],
            "wma",
        ),
        (&["audio/mpeg", "audio/mp3", "audio/x-mp3"], "mp3"),
        (&["audio/ogg", "application/ogg"], "ogg"),
        (
            &["audio/mp4", "audio/x-m4a", "audio/aac", "audio/aacp"],
            "m4a",
        ),
    ];
    table
        .into_iter()
        .find(|(patterns, _)| patterns.contains(&value.as_str()))
        .map(|(_, extension)| extension)
}

/// Signature, then Content-Type, then `mp3`.
pub fn detect_extension(data: &[u8], content_type: Option<&str>) -> &'static str {
    detect_by_signature(data)
        .or_else(|| content_type.and_then(detect_by_content_type))
        .unwrap_or("mp3")
}

/// Canonical MIME type for a container.
pub fn mime_by_extension(extension: &str) -> &'static str {
    match extension {
        "wma" => "audio/x-ms-wma",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        _ => "audio/mpeg",
    }
}

/// True when a Content-Type carries no useful information and should be
/// replaced by the extension-derived MIME type.
pub fn content_type_is_opaque(content_type: Option<&str>) -> bool {
    match content_type {
        None => true,
        Some(value) => {
            let value = value.trim().to_ascii_lowercase();
            value.is_empty() || value.starts_with("application/octet-stream")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        content_type_is_opaque, detect_by_content_type, detect_by_signature, detect_extension,
        is_taggable, mime_by_extension, ASF_GUID,
    };

    #[test]
    fn detects_containers_by_magic_number() {
        assert_eq!(detect_by_signature(b"fLaC\0\0\0\0"), Some("flac"));
        assert_eq!(detect_by_signature(b"ID3\x03\0\0\0\0"), Some("mp3"));
        assert_eq!(detect_by_signature(&[0xFF, 0xFB, 0x90, 0x00]), Some("mp3"));
        assert_eq!(detect_by_signature(b"OggS\0\0\0\0"), Some("ogg"));
        assert_eq!(detect_by_signature(b"\0\0\0\x18ftypM4A "), Some("m4a"));
        assert_eq!(detect_by_signature(&ASF_GUID), Some("wma"));
    }

    #[test]
    fn unknown_and_short_input_is_none() {
        assert_eq!(detect_by_signature(b"RIFF"), None);
        assert_eq!(detect_by_signature(&[0xFF]), None);
        assert_eq!(detect_by_signature(&[]), None);
    }

    #[test]
    fn content_type_parameters_are_ignored() {
        assert_eq!(
            detect_by_content_type("audio/mpeg; charset=utf-8"),
            Some("mp3")
        );
        assert_eq!(detect_by_content_type("AUDIO/FLAC"), Some("flac"));
        assert_eq!(detect_by_content_type("text/html"), None);
    }

    #[test]
    fn signature_beats_content_type_and_mp3_is_the_fallback() {
        assert_eq!(detect_extension(b"fLaC", Some("audio/mpeg")), "flac");
        assert_eq!(detect_extension(b"????", Some("audio/flac")), "flac");
        assert_eq!(detect_extension(b"????", None), "mp3");
    }

    #[test]
    fn octet_stream_and_missing_type_are_opaque() {
        assert!(content_type_is_opaque(None));
        assert!(content_type_is_opaque(Some("application/octet-stream")));
        assert!(!content_type_is_opaque(Some("audio/flac")));
    }

    #[test]
    fn ogg_is_detected_but_not_taggable() {
        assert_eq!(mime_by_extension("ogg"), "audio/ogg");
        assert!(!is_taggable("ogg"));
        assert!(is_taggable("flac"));
    }
}
