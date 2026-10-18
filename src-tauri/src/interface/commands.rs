use std::sync::Arc;

use crate::application::search::{
    aggregate_collections, aggregate_search, AggregatedCollections, AggregatedSearch,
    CollectionKind, SourceFailure, SOURCE_TIMEOUT,
};
use crate::application::switch::{
    find_alternative, resolve_with_fallback, Playability, PlayabilityReport, ResolvedPlayback,
};
use crate::domain::collection::CollectionQuery;
use crate::domain::link::detect_source;
use crate::domain::media::{Lyrics, PlayableResource};
use crate::domain::source::{
    CollectionDetail, MusicSource, SearchQuery, Song, SongRef, SourceCapabilities, SourceError,
};
use crate::error::IpcError;
use crate::infrastructure::sources::registry::SourceRegistry;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use tauri::State;

#[tauri::command]
pub fn health() -> Result<&'static str, IpcError> {
    Ok("ok")
}

#[tauri::command]
pub fn validate_search(request: SearchQuery) -> Result<SearchQuery, IpcError> {
    if request.query.trim().is_empty() {
        return Err(IpcError::new(
            "INVALID_ARGUMENT",
            "搜索关键词不能为空",
            false,
            "validation".to_owned(),
        ));
    }
    Ok(request)
}

#[tauri::command]
pub fn validate_collection(request: CollectionQuery) -> Result<CollectionQuery, IpcError> {
    if request.query.trim().is_empty() || request.page == 0 || request.page_size == 0 {
        return Err(IpcError::new(
            "INVALID_ARGUMENT",
            "歌单/专辑查询参数无效",
            false,
            "validation".to_owned(),
        ));
    }
    Ok(request)
}

#[tauri::command]
pub fn normalize_lyrics_command(request: Lyrics) -> Result<Lyrics, IpcError> {
    if request.original.is_none() && request.translated.is_none() && request.romanized.is_none() {
        return Err(IpcError::new(
            "INVALID_ARGUMENT",
            "歌词内容不能为空",
            false,
            "validation".to_owned(),
        ));
    }
    Ok(request)
}

/// Concurrent multi-source song search.
///
/// Partial failure is not an error: unknown or failing sources land in
/// `sourceErrors` so the UI can report "3 of 5 sources responded".
#[tauri::command]
pub async fn search_songs(
    state: State<'_, AppState>,
    query: String,
    sources: Vec<String>,
    page: u32,
    page_size: u32,
) -> Result<AggregatedSearch, IpcError> {
    let (resolved, missing) = resolve_sources(&state, &sources);
    let mut result = aggregate_search(
        resolved,
        SearchQuery {
            query,
            page,
            page_size,
            sources: sources.clone(),
        },
        SOURCE_TIMEOUT,
    )
    .await
    .map_err(|error| ipc(error, "search_songs"))?;
    for failure in missing {
        result.record_failure(failure);
    }
    Ok(result)
}

/// Concurrent playlist or album search across sources.
#[tauri::command]
pub async fn search_collections(
    state: State<'_, AppState>,
    query: String,
    kind: CollectionKind,
    sources: Vec<String>,
    page: u32,
    page_size: u32,
) -> Result<AggregatedCollections, IpcError> {
    let (resolved, missing) = resolve_sources(&state, &sources);
    let mut result = aggregate_collections(
        resolved,
        SearchQuery {
            query,
            page,
            page_size,
            sources: sources.clone(),
        },
        kind,
        SOURCE_TIMEOUT,
    )
    .await
    .map_err(|error| ipc(error, "search_collections"))?;
    for failure in missing {
        result.record_failure(failure);
    }
    Ok(result)
}

/// Tracks of one playlist or album.
#[tauri::command]
pub async fn get_collection_detail(
    state: State<'_, AppState>,
    source: String,
    kind: CollectionKind,
    id: String,
) -> Result<CollectionDetail, IpcError> {
    if id.trim().is_empty() {
        return Err(IpcError::new(
            "INVALID_ARGUMENT",
            "歌单/专辑 ID 不能为空",
            false,
            "get_collection_detail".to_owned(),
        ));
    }
    let adapter = source_or_error(&state, &source, "get_collection_detail")?;
    let detail = match kind {
        CollectionKind::Playlist => adapter.playlist_detail(&id).await,
        CollectionKind::Album => adapter.album_detail(&id).await,
    };
    detail.map_err(|error| ipc(error, "get_collection_detail"))
}

/// Songs behind a share link.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedShareLink {
    pub source: String,
    pub kind: CollectionKind,
    pub id: String,
    /// Present for playlist/album links; `None` for a single song.
    pub collection: Option<crate::domain::collection::Collection>,
    pub songs: Vec<Song>,
}

/// Resolves a pasted share URL into songs.
///
/// The source is recognised first, then playlist and album detail are tried in
/// turn, matching the TUI's link handling (scout-app §2.5): one URL shape cannot
/// be told apart from the other without asking the platform.
#[tauri::command]
pub async fn parse_share_link(
    state: State<'_, AppState>,
    url: String,
) -> Result<ParsedShareLink, IpcError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(IpcError::new(
            "INVALID_ARGUMENT",
            "分享链接不能为空",
            false,
            "parse_share_link".to_owned(),
        ));
    }
    let source = detect_source(trimmed).ok_or_else(|| {
        IpcError::new(
            "LINK_UNRECOGNISED",
            "无法识别该分享链接所属平台",
            false,
            "parse_share_link".to_owned(),
        )
    })?;
    let adapter = source_or_error(&state, source, "parse_share_link")?;
    let id = extract_link_id(trimmed).ok_or_else(|| {
        IpcError::new(
            "LINK_UNRECOGNISED",
            "分享链接中缺少可解析的 ID",
            false,
            "parse_share_link".to_owned(),
        )
    })?;

    let mut last: Option<SourceError> = None;
    for kind in [CollectionKind::Playlist, CollectionKind::Album] {
        let detail = match kind {
            CollectionKind::Playlist => adapter.playlist_detail(&id).await,
            CollectionKind::Album => adapter.album_detail(&id).await,
        };
        match detail {
            Ok(detail) if !detail.songs.is_empty() => {
                return Ok(ParsedShareLink {
                    source: source.to_owned(),
                    kind,
                    id,
                    collection: Some(detail.collection),
                    songs: detail.songs,
                });
            }
            Ok(_) => {}
            Err(error) => last = Some(error),
        }
    }

    // Not a collection: treat the id as a single track and confirm it resolves.
    let reference = SongRef {
        source: source.to_owned(),
        scope: "song".to_owned(),
        site_id: id.clone(),
    };
    match adapter.resolve_song(reference.clone()).await {
        Ok(resource) => Ok(ParsedShareLink {
            source: source.to_owned(),
            kind: CollectionKind::Playlist,
            id,
            collection: None,
            songs: vec![Song {
                reference,
                title: String::new(),
                artists: Vec::new(),
                album: None,
                duration_ms: resource.duration_ms,
                cover_url: None,
            }],
        }),
        Err(error) => Err(ipc(last.unwrap_or(error), "parse_share_link")),
    }
}

/// Lyrics for one song, from its own source.
#[tauri::command]
pub async fn get_lyrics(state: State<'_, AppState>, song: SongRef) -> Result<Lyrics, IpcError> {
    if song.site_id.trim().is_empty() {
        return Err(IpcError::new(
            "INVALID_ARGUMENT",
            "歌曲引用无效",
            false,
            "get_lyrics".to_owned(),
        ));
    }
    let adapter = source_or_error(&state, &song.source, "get_lyrics")?;
    adapter
        .lyrics(song)
        .await
        .map_err(|error| ipc(error, "get_lyrics"))
}

/// One source's id, capabilities and login state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDescriptor {
    pub id: String,
    pub capabilities: SourceCapabilities,
    /// Whether a cookie is currently stored for this source.
    pub authenticated: bool,
}

/// Every registered source with its capabilities and login state.
#[tauri::command]
pub fn get_source_capabilities(
    state: State<'_, AppState>,
) -> Result<Vec<SourceDescriptor>, IpcError> {
    Ok(state
        .sources
        .ids()
        .into_iter()
        .filter_map(|id| {
            let adapter = state.sources.get(&id)?;
            Some(SourceDescriptor {
                capabilities: adapter.capabilities(),
                authenticated: !state.cookies.get(&id).is_empty(),
                id,
            })
        })
        .collect())
}

/// Resolves a playable URL, switching source when the primary cannot serve the
/// audio and `autoSwitchInvalidSources` is on.
#[tauri::command]
pub async fn resolve_playback(
    state: State<'_, AppState>,
    song: Song,
) -> Result<ResolvedPlayback, IpcError> {
    if song.reference.scope != "song" || song.reference.site_id.trim().is_empty() {
        return Err(IpcError::new(
            "INVALID_ARGUMENT",
            "歌曲引用无效",
            false,
            "resolve_playback".to_owned(),
        ));
    }
    let auto_switch = state.settings_snapshot().auto_switch_invalid_sources;
    let sources: Vec<Arc<dyn MusicSource>> = state
        .sources
        .ids()
        .iter()
        .filter_map(|id| state.sources.get(id))
        .collect();
    let fetcher = Arc::new(state.http.clone());
    resolve_with_fallback(
        sources,
        fetcher,
        Arc::clone(&state.cookies),
        &song,
        auto_switch,
    )
    .await
    .map_err(|error| ipc(error, "resolve_playback"))
}

/// Result of a user-initiated source switch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSwitch {
    /// Source the user switched away from.
    pub from: String,
    /// Source that actually served the alternative.
    pub served_by: String,
    /// Verified stream on `servedBy`; `resource.song` is the new reference the
    /// UI plays and re-resolves against.
    pub resource: PlayableResource,
    pub playability: Playability,
    /// Candidates probed and rejected before the winner, best-ranked first.
    pub rejected: Vec<PlayabilityReport>,
}

/// Finds an equivalent recording on a *different* source, on user request.
///
/// An explicit "this track is broken, get me another" action. It is
/// deliberately not wired into download or playback resolution — those
/// resolve on the requested source only.
#[tauri::command]
pub async fn switch_source(
    state: State<'_, AppState>,
    song: Song,
) -> Result<SourceSwitch, IpcError> {
    if song.title.trim().is_empty() {
        return Err(IpcError::new(
            "INVALID_ARGUMENT",
            "歌曲标题为空，无法换源",
            false,
            "switch_source".to_owned(),
        ));
    }
    let current = song.reference.source.clone();
    let candidates = switch_candidate_sources(&state.sources, &current);
    if candidates.is_empty() {
        return Err(IpcError::new(
            "SWITCH_NO_CANDIDATE_SOURCE",
            &format!("除了 {current} 之外没有其他可用音源"),
            false,
            "switch_source".to_owned(),
        ));
    }

    tracing::info!(event = "switch_source_requested", from = %current, title = %song.title);
    let fetcher = Arc::new(state.http.clone());
    let search = find_alternative(candidates, fetcher, Arc::clone(&state.cookies), &song).await;
    // A winner without a resolved resource cannot be played, so it is a failed
    // switch rather than a success the UI would choke on.
    let Some((served_by, playability, resource)) = search
        .winner
        .and_then(|report| Some((report.source, report.playability, report.resource?)))
    else {
        tracing::warn!(
            event = "switch_source_exhausted",
            from = %current,
            rejected = search.attempts.len()
        );
        return Err(IpcError::new(
            "SWITCH_NO_PLAYABLE_SOURCE",
            &format!("{} 没有找到可用的替代音源", song.title),
            true,
            "switch_source".to_owned(),
        ));
    };

    tracing::info!(event = "switch_source_succeeded", from = %current, to = %served_by);
    Ok(SourceSwitch {
        from: current,
        served_by,
        resource,
        playability,
        rejected: search.attempts,
    })
}

/// Adapters eligible to serve a switch: every registered source except the one
/// the user is already on.
///
/// Dropping the current source here, rather than trusting the candidate search
/// to skip it, keeps "a switch changes source" true of this command itself.
fn switch_candidate_sources(registry: &SourceRegistry, current: &str) -> Vec<Arc<dyn MusicSource>> {
    registry
        .ids()
        .iter()
        .filter(|id| id.as_str() != current)
        .filter_map(|id| registry.get(id))
        .collect()
}

/// Splits requested ids into live adapters and "not found" failures, so an
/// unknown id is reported per-source instead of failing the whole search.
fn resolve_sources(
    state: &State<'_, AppState>,
    requested: &[String],
) -> (Vec<Arc<dyn MusicSource>>, Vec<SourceFailure>) {
    let mut adapters = Vec::with_capacity(requested.len());
    let mut missing = Vec::new();
    for id in requested {
        match state.sources.get(id) {
            Some(adapter) => adapters.push(adapter),
            None => missing.push(SourceFailure::not_found(id.clone())),
        }
    }
    (adapters, missing)
}

fn source_or_error(
    state: &State<'_, AppState>,
    id: &str,
    operation: &str,
) -> Result<Arc<dyn MusicSource>, IpcError> {
    state.sources.get(id).ok_or_else(|| {
        IpcError::new(
            "SOURCE_NOT_FOUND",
            "音乐源不存在",
            false,
            operation.to_owned(),
        )
    })
}

/// Last path segment or `id`/`playlist` query parameter of a share URL.
fn extract_link_id(url: &str) -> Option<String> {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let (path, query) = match without_fragment.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (without_fragment, None),
    };
    if let Some(query) = query {
        for pair in query.split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            if matches!(key, "id" | "playlist" | "album" | "aid" | "bvid") && !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    path.trim_end_matches('/')
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .map(|segment| segment.to_owned())
}

fn ipc(error: SourceError, operation: &str) -> IpcError {
    IpcError::new(
        &error.code,
        &error.message,
        error.retryable,
        operation.to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::switch_candidate_sources;
    use crate::infrastructure::credentials::CookieStore;
    use crate::infrastructure::sources::registry::{SourceRegistry, SOURCE_IDS};
    use std::sync::Arc;

    #[test]
    fn current_source_is_never_a_switch_candidate() {
        let registry = SourceRegistry::with_sources(Arc::new(CookieStore::default()));
        let ids: Vec<String> = switch_candidate_sources(&registry, "netease")
            .iter()
            .map(|source| source.id().to_owned())
            .collect();
        assert!(!ids.contains(&"netease".to_owned()));
        assert_eq!(ids.len(), SOURCE_IDS.len() - 1);
    }
}
