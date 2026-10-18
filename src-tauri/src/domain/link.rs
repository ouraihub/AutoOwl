//! Share-link recognition and canonical link building.
//!
//! Source detection is order sensitive: `5sing` is matched before `kugou.com`
//! because the 5sing domain is `5sing.kugou.com`.

/// Content kind a link or id refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    Song,
    Album,
    Playlist,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Song => "song",
            Self::Album => "album",
            Self::Playlist => "playlist",
        }
    }
}

/// Maps a share URL to a source id, or `None` when unrecognised.
pub fn detect_source(url: &str) -> Option<&'static str> {
    let value = url.to_lowercase();
    let rules: [(&[&str], &str); 12] = [
        (&["163.com"], "netease"),
        (&["qq.com"], "qq"),
        (&["5sing"], "fivesing"),
        (&["kugou.com"], "kugou"),
        (&["kuwo.cn"], "kuwo"),
        (&["migu.cn"], "migu"),
        (&["joox.com"], "joox"),
        (&["bilibili.com", "b23.tv"], "bilibili"),
        (&["douyin.com", "qishui"], "soda"),
        (&["91q.com"], "qianqian"),
        (&["jamendo.com"], "jamendo"),
        (&["music.apple.com", "itunes.apple.com"], "apple"),
    ];
    rules
        .into_iter()
        .find(|(patterns, _)| patterns.iter().any(|pattern| value.contains(pattern)))
        .map(|(_, source)| source)
}

/// Builds the canonical web link for a source item, empty when unsupported.
pub fn original_link(source: &str, id: &str, kind: LinkKind) -> String {
    match source {
        "netease" => match kind {
            LinkKind::Album => format!("https://music.163.com/#/album?id={id}"),
            LinkKind::Playlist => format!("https://music.163.com/#/playlist?id={id}"),
            LinkKind::Song => format!("https://music.163.com/#/song?id={id}"),
        },
        "qq" => match kind {
            LinkKind::Album => format!("https://y.qq.com/n/ryqq/albumDetail/{id}"),
            LinkKind::Playlist => {
                if id.starts_with("profile:") {
                    "https://y.qq.com/n/ryqq/profile".to_owned()
                } else {
                    format!("https://y.qq.com/n/ryqq/playlist/{id}")
                }
            }
            LinkKind::Song => format!("https://y.qq.com/n/ryqq/songDetail/{id}"),
        },
        "kugou" => match kind {
            LinkKind::Album => format!("https://www.kugou.com/album/{id}.html"),
            LinkKind::Playlist => {
                if id.starts_with("cloudlist:") {
                    String::new()
                } else {
                    format!("https://www.kugou.com/yy/special/single/{id}.html")
                }
            }
            LinkKind::Song => format!("https://www.kugou.com/song/#hash={id}"),
        },
        "kuwo" => match kind {
            LinkKind::Album => format!("http://www.kuwo.cn/album_detail/{id}"),
            LinkKind::Playlist => format!("http://www.kuwo.cn/playlist_detail/{id}"),
            LinkKind::Song => format!("http://www.kuwo.cn/play_detail/{id}"),
        },
        "migu" => match kind {
            LinkKind::Album => format!("https://music.migu.cn/v3/music/album/{id}"),
            LinkKind::Playlist => {
                format!("https://music.migu.cn/v5/#/playlist?playlistId={id}&playlistType=ordinary")
            }
            LinkKind::Song => format!("https://music.migu.cn/v3/music/song/{id}"),
        },
        "jamendo" => match kind {
            LinkKind::Album => format!("https://www.jamendo.com/album/{id}"),
            LinkKind::Playlist => format!("https://www.jamendo.com/playlist/{id}"),
            LinkKind::Song => format!("https://www.jamendo.com/track/{id}"),
        },
        "joox" => match kind {
            LinkKind::Album => format!("https://www.joox.com/hk/album/{id}"),
            LinkKind::Playlist => format!("https://www.joox.com/hk/playlist/{id}"),
            LinkKind::Song => format!("https://www.joox.com/hk/single/{id}"),
        },
        "qianqian" => match kind {
            LinkKind::Album => format!("https://music.91q.com/album/{id}"),
            LinkKind::Playlist => format!("https://music.91q.com/songlist/{id}"),
            LinkKind::Song => format!("https://music.91q.com/song/{id}"),
        },
        "soda" => match kind {
            LinkKind::Album => format!("https://www.qishui.com/share/album?album_id={id}"),
            LinkKind::Playlist => format!("https://www.qishui.com/playlist/{id}"),
            LinkKind::Song => String::new(),
        },
        "bilibili" => format!("https://www.bilibili.com/video/{id}"),
        "apple" => format!("https://music.apple.com/{}/{id}", kind.as_str()),
        "fivesing" => match kind {
            LinkKind::Playlist => format!("http://5sing.kugou.com/dj/{id}.html"),
            LinkKind::Song if id.contains('/') => format!("http://5sing.kugou.com/{id}.html"),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{detect_source, original_link, LinkKind};

    #[test]
    fn fivesing_wins_over_kugou_domain() {
        assert_eq!(
            detect_source("http://5sing.kugou.com/yc/4114875.html"),
            Some("fivesing")
        );
        assert_eq!(
            detect_source("https://www.kugou.com/song/#hash=ABC"),
            Some("kugou")
        );
    }

    #[test]
    fn short_and_alias_domains_are_recognised() {
        assert_eq!(detect_source("https://b23.tv/abc123"), Some("bilibili"));
        assert_eq!(
            detect_source("https://www.qishui.com/share/album?album_id=1"),
            Some("soda")
        );
        assert_eq!(detect_source("https://v.douyin.com/xyz/"), Some("soda"));
        assert_eq!(
            detect_source("https://itunes.apple.com/album/1"),
            Some("apple")
        );
    }

    #[test]
    fn unknown_host_is_none() {
        assert_eq!(detect_source("https://example.com/song/1"), None);
    }

    #[test]
    fn qq_profile_playlist_collapses_to_profile_page() {
        assert_eq!(
            original_link("qq", "profile:123", LinkKind::Playlist),
            "https://y.qq.com/n/ryqq/profile"
        );
        assert_eq!(
            original_link("qq", "123", LinkKind::Playlist),
            "https://y.qq.com/n/ryqq/playlist/123"
        );
    }

    #[test]
    fn kugou_cloudlist_playlist_has_no_link() {
        assert!(original_link("kugou", "cloudlist:9", LinkKind::Playlist).is_empty());
    }

    #[test]
    fn bilibili_ignores_kind() {
        let song = original_link("bilibili", "BV1", LinkKind::Song);
        assert_eq!(song, "https://www.bilibili.com/video/BV1");
        assert_eq!(original_link("bilibili", "BV1", LinkKind::Album), song);
    }

    #[test]
    fn fivesing_song_requires_type_prefix() {
        assert_eq!(
            original_link("fivesing", "yc/123", LinkKind::Song),
            "http://5sing.kugou.com/yc/123.html"
        );
        assert!(original_link("fivesing", "123", LinkKind::Song).is_empty());
    }

    #[test]
    fn unknown_source_has_no_link() {
        assert!(original_link("local", "1", LinkKind::Song).is_empty());
    }
}
