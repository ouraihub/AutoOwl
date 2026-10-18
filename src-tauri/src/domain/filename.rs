use std::path::{Path, PathBuf};

use super::source::Song;

pub fn render(template: &str, song: &Song, extension: &str) -> PathBuf {
    let artist = sanitize(
        song.artists
            .first()
            .map(|item| item.name.as_str())
            .unwrap_or("Unknown"),
    );
    let album = sanitize(
        song.album
            .as_ref()
            .map(|item| item.name.as_str())
            .unwrap_or(""),
    );
    let rendered = template
        .replace("{name}", &sanitize(&song.title))
        .replace("{artist}", &artist)
        .replace("{album}", &album)
        .replace("{source}", &sanitize(&song.reference.source))
        .replace("{id}", &sanitize(&song.reference.site_id))
        .replace("{ext}", &sanitize(extension));
    rendered
        .split(['/', '\\'])
        .filter(|part| *part != "." && *part != ".." && !part.is_empty())
        .map(sanitize)
        .filter(|part| !part.is_empty())
        .collect::<PathBuf>()
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => ch,
        })
        .collect()
}

pub fn target(root: &Path, template: &str, song: &Song, extension: &str) -> PathBuf {
    let mut relative = render(template, song, extension);
    if relative.extension().is_none() {
        relative.set_extension(sanitize(extension));
    }
    root.join(relative)
}

#[cfg(test)]
mod tests {
    use super::target;
    use crate::domain::source::{AlbumRef, Artist, Song, SongRef};
    use std::path::Path;

    #[test]
    fn renders_template_without_traversal() {
        let song = Song {
            reference: SongRef {
                source: "test".into(),
                scope: "song".into(),
                site_id: "1".into(),
            },
            title: "A/B".into(),
            artists: vec![Artist {
                id: None,
                name: "Artist".into(),
            }],
            album: Some(AlbumRef {
                id: None,
                name: "Album".into(),
                source: None,
            }),
            duration_ms: None,
            cover_url: None,
        };
        let path = target(
            Path::new("downloads"),
            "{artist}/{name}.{ext}",
            &song,
            "mp3",
        );
        assert_eq!(path, Path::new("downloads/Artist/A_B.mp3"));
    }
}
