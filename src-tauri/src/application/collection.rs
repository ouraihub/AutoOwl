//! Collection search/detail use cases.
//!
//! Thin rules layer over `MusicSource::search_playlists` / `search_albums` and
//! the detail calls; the stored "my collections" side lives in
//! `collection_use_case`.

use crate::domain::collection::{CollectionQuery, CollectionType};
use crate::domain::source::SearchQuery;

/// Largest page a collection search will serve.
pub const MAX_PAGE_SIZE: u32 = 100;

pub fn validate_collection_query(query: &CollectionQuery) -> Result<(), String> {
    if query.query.trim().is_empty() {
        return Err("搜索关键词不能为空".to_owned());
    }
    if query.page == 0 || query.page_size == 0 || query.page_size > MAX_PAGE_SIZE {
        return Err(format!(
            "分页参数无效，页码需大于 0 且每页 1-{MAX_PAGE_SIZE} 条"
        ));
    }
    Ok(())
}

/// Adapts a collection query to the shared source query shape.
pub fn to_search_query(query: &CollectionQuery, sources: Vec<String>) -> SearchQuery {
    SearchQuery {
        query: query.query.trim().to_owned(),
        page: query.page,
        page_size: query.page_size.min(MAX_PAGE_SIZE),
        sources,
    }
}

/// Parses the frontend's collection kind string.
pub fn parse_content_type(raw: &str) -> Result<CollectionType, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "playlist" | "" => Ok(CollectionType::Playlist),
        "album" => Ok(CollectionType::Album),
        other => Err(format!("未知的歌单类型: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_content_type, to_search_query, validate_collection_query, MAX_PAGE_SIZE};
    use crate::domain::collection::{CollectionQuery, CollectionType};

    fn query(text: &str, page: u32, page_size: u32) -> CollectionQuery {
        CollectionQuery {
            query: text.to_owned(),
            page,
            page_size,
        }
    }

    #[test]
    fn blank_keywords_and_bad_pages_are_rejected() {
        assert!(validate_collection_query(&query("周杰伦", 1, 20)).is_ok());
        assert!(validate_collection_query(&query("   ", 1, 20)).is_err());
        assert!(validate_collection_query(&query("周杰伦", 0, 20)).is_err());
        assert!(validate_collection_query(&query("周杰伦", 1, 0)).is_err());
        assert!(validate_collection_query(&query("周杰伦", 1, MAX_PAGE_SIZE + 1)).is_err());
    }

    #[test]
    fn search_query_trims_the_keyword_and_clamps_the_page_size() {
        let adapted = to_search_query(&query("  周杰伦  ", 2, 1000), vec!["netease".into()]);
        assert_eq!(adapted.query, "周杰伦");
        assert_eq!(adapted.page, 2);
        assert_eq!(adapted.page_size, MAX_PAGE_SIZE);
        assert_eq!(adapted.sources, ["netease"]);
    }

    #[test]
    fn content_type_defaults_to_playlist_and_rejects_unknown() {
        assert!(matches!(
            parse_content_type("playlist"),
            Ok(CollectionType::Playlist)
        ));
        assert!(matches!(
            parse_content_type(""),
            Ok(CollectionType::Playlist)
        ));
        assert!(matches!(
            parse_content_type("Album"),
            Ok(CollectionType::Album)
        ));
        assert!(parse_content_type("single").is_err());
    }
}
