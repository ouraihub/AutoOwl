//! Multi-source search aggregation.
//!
//! Queries every selected source concurrently, survives partial failure, ranks
//! the merged rows with the shared similarity primitives and collapses the same
//! track appearing on several platforms.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use crate::domain::similarity::{
    duration_close, normalize_text, similarity_score, song_similarity,
};
use crate::domain::source::{MusicSource, SearchQuery, Song, SongRef, SourceError};

/// Per-source deadline. A dead platform must not stall the whole search.
pub const SOURCE_TIMEOUT: Duration = Duration::from_secs(10);

/// Why one source contributed nothing, kept alongside the results so the UI can
/// report "3 of 5 sources responded" instead of failing the whole search.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFailure {
    pub source: String,
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl SourceFailure {
    fn from_error(source: String, error: SourceError) -> Self {
        Self {
            source,
            code: error.code,
            message: error.message,
            retryable: error.retryable,
        }
    }

    fn timeout(source: String, timeout: Duration) -> Self {
        Self {
            message: format!("{source} 搜索超时（{} 秒）", timeout.as_secs()),
            source,
            code: "SOURCE_TIMEOUT".to_owned(),
            retryable: true,
        }
    }

    pub fn not_found(source: String) -> Self {
        Self {
            message: format!("音乐源不存在：{source}"),
            source,
            code: "SOURCE_NOT_FOUND".to_owned(),
            retryable: false,
        }
    }

    fn panicked(source: String) -> Self {
        Self {
            message: format!("{source} 搜索任务异常终止"),
            source,
            code: "SOURCE_INTERNAL_ERROR".to_owned(),
            retryable: true,
        }
    }
}

/// Another platform serving the same track, used by the player to switch source
/// without searching again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlternativeSource {
    pub source: String,
    #[serde(rename = "ref")]
    pub reference: SongRef,
    pub score: f64,
}

/// A merged search row: the best-ranked copy of a track plus every other source
/// that also carries it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedSong {
    pub song: Song,
    pub score: f64,
    pub alternatives: Vec<AlternativeSource>,
}

/// Aggregated result. Additive over [`crate::domain::source::SearchResult`]:
/// same paging fields, plus ranking, alternatives and per-source errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedSearch {
    pub items: Vec<RankedSong>,
    /// Sum of the per-source upstream totals, so this counts pre-dedupe
    /// availability rather than `items.len()`.
    pub total: Option<u64>,
    pub page: u32,
    pub page_size: u32,
    pub source_errors: Vec<SourceFailure>,
    pub queried_sources: usize,
    pub responded_sources: usize,
}

impl AggregatedSearch {
    /// Records a source that never got queried, keeping the "N of M responded"
    /// counters honest.
    pub fn record_failure(&mut self, failure: SourceFailure) {
        self.queried_sources += 1;
        self.source_errors.push(failure);
    }
}

/// Queries every source concurrently and merges the results.
///
/// Fails only when no source produced a result; any other combination returns
/// the rows that did arrive with the rest reported in `source_errors`.
pub async fn aggregate_search(
    sources: Vec<Arc<dyn MusicSource>>,
    query: SearchQuery,
    timeout: Duration,
) -> Result<AggregatedSearch, SourceError> {
    if query.query.trim().is_empty() {
        return Err(invalid("搜索关键词不能为空"));
    }
    if query.page == 0 || query.page_size == 0 {
        return Err(invalid("分页参数无效"));
    }
    if sources.is_empty() {
        return Err(invalid("至少选择一个音乐源"));
    }

    let queried_sources = sources.len();
    let mut tasks = JoinSet::new();
    for source in sources {
        let mut per_source = query.clone();
        per_source.sources = vec![source.id().to_owned()];
        tasks.spawn(async move {
            let id = source.id().to_owned();
            let outcome = tokio::time::timeout(timeout, source.search_songs(per_source)).await;
            (id, outcome)
        });
    }

    let mut harvested: Vec<Song> = Vec::new();
    let mut source_errors = Vec::new();
    let mut total = 0_u64;
    let mut responded_sources = 0_usize;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((id, Ok(Ok(result)))) => {
                responded_sources += 1;
                total = total.saturating_add(result.total.unwrap_or(result.items.len() as u64));
                tracing::debug!(
                    event = "source_search_ok",
                    source = %id,
                    items = result.items.len()
                );
                harvested.extend(result.items);
            }
            Ok((id, Ok(Err(error)))) => {
                tracing::warn!(event = "source_search_failed", source = %id, code = %error.code);
                source_errors.push(SourceFailure::from_error(id, error));
            }
            Ok((id, Err(_elapsed))) => {
                tracing::warn!(event = "source_search_timeout", source = %id);
                source_errors.push(SourceFailure::timeout(id, timeout));
            }
            Err(join_error) => {
                // The id is lost with the task, so the failure is attributed to
                // the batch rather than dropped silently.
                tracing::warn!(event = "source_search_panicked", %join_error);
                source_errors.push(SourceFailure::panicked("unknown".to_owned()));
            }
        }
    }

    if responded_sources == 0 {
        return Err(all_failed(&source_errors));
    }

    Ok(AggregatedSearch {
        items: merge_ranked(&query.query, harvested),
        total: Some(total),
        page: query.page,
        page_size: query.page_size,
        source_errors,
        queried_sources,
        responded_sources,
    })
}

/// Ranks candidates against the keyword, then collapses duplicates across
/// sources keeping the best-ranked copy.
fn merge_ranked(keyword: &str, candidates: Vec<Song>) -> Vec<RankedSong> {
    let mut scored: Vec<(f64, Song)> = candidates
        .into_iter()
        .map(|song| (score_song(keyword, &song), song))
        .collect();
    // JoinSet completion order is nondeterministic, so the tie-break makes the
    // merged list reproducible for the same inputs.
    scored.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.1.reference.source.cmp(&right.1.reference.source))
            .then_with(|| left.1.reference.site_id.cmp(&right.1.reference.site_id))
    });

    let mut merged: Vec<RankedSong> = Vec::new();
    // Duration proximity is not transitive, so a key can hold several distinct
    // groups; the index keeps the candidate comparison out of an O(n^2) scan.
    let mut index: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (score, song) in scored {
        let slots = index.entry(dedupe_key(&song)).or_default();
        let existing = slots
            .iter()
            .copied()
            .find(|&at| durations_match(&merged[at].song, &song));
        match existing {
            Some(at) => merged[at].alternatives.push(AlternativeSource {
                source: song.reference.source.clone(),
                reference: song.reference,
                score,
            }),
            None => {
                slots.push(merged.len());
                merged.push(RankedSong {
                    song,
                    score,
                    alternatives: Vec::new(),
                });
            }
        }
    }
    merged
}

/// Scores a candidate against a free-text keyword.
///
/// The keyword is one field, so besides the title-only score it is also matched
/// against both "title artist" orders; that is what lets a row matching title
/// *and* artist outrank a row matching the title alone.
pub fn score_song(keyword: &str, song: &Song) -> f64 {
    let artists = artist_names(song);
    let title_only = song_similarity(keyword, "", &song.title, "");
    if artists.is_empty() {
        return title_only;
    }
    let title_first = similarity_score(keyword, &format!("{} {}", song.title, artists));
    let artist_first = similarity_score(keyword, &format!("{} {}", artists, song.title));
    title_only.max(title_first).max(artist_first)
}

/// Comma-free artist list used for scoring and dedupe.
pub fn artist_names(song: &Song) -> String {
    song.artists
        .iter()
        .map(|artist| artist.name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalized title plus order-insensitive artist set.
///
/// Sources list collaborators in different orders, so the names are sorted
/// before normalizing; otherwise the same track would not collapse.
fn dedupe_key(song: &Song) -> (String, String) {
    let mut names: Vec<String> = song
        .artists
        .iter()
        .map(|artist| normalize_text(&artist.name))
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    (normalize_text(&song.title), names.join("|"))
}

fn durations_match(left: &Song, right: &Song) -> bool {
    duration_close(
        left.duration_ms.unwrap_or(0),
        right.duration_ms.unwrap_or(0),
    )
}

fn invalid(message: &str) -> SourceError {
    SourceError {
        code: "INVALID_ARGUMENT".to_owned(),
        message: message.to_owned(),
        retryable: false,
    }
}

/// Collapses per-source failures into one error for the "nothing responded"
/// case, preserving the single-source code when only one source was asked.
fn all_failed(failures: &[SourceFailure]) -> SourceError {
    if let [single] = failures {
        return SourceError {
            code: single.code.clone(),
            message: single.message.clone(),
            retryable: single.retryable,
        };
    }
    if failures.is_empty() {
        return SourceError {
            code: "SEARCH_ALL_SOURCES_FAILED".to_owned(),
            message: "搜索失败：没有音乐源返回结果".to_owned(),
            retryable: true,
        };
    }
    let names: Vec<&str> = failures
        .iter()
        .map(|failure| failure.source.as_str())
        .collect();
    SourceError {
        code: "SEARCH_ALL_SOURCES_FAILED".to_owned(),
        message: format!("全部音乐源搜索失败：{}", names.join("、")),
        retryable: failures.iter().any(|failure| failure.retryable),
    }
}

/// Which collection listing to fan out to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CollectionKind {
    Playlist,
    Album,
}

impl CollectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Playlist => "playlist",
            Self::Album => "album",
        }
    }
}

/// Aggregated playlist/album search, mirroring [`AggregatedSearch`] so the UI
/// can render both with the same "N of M responded" affordance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedCollections {
    pub items: Vec<crate::domain::collection::Collection>,
    pub total: Option<u64>,
    pub page: u32,
    pub page_size: u32,
    pub source_errors: Vec<SourceFailure>,
    pub queried_sources: usize,
    pub responded_sources: usize,
}

impl AggregatedCollections {
    pub fn record_failure(&mut self, failure: SourceFailure) {
        self.queried_sources += 1;
        self.source_errors.push(failure);
    }
}

/// Concurrent playlist/album search across sources.
///
/// Sources lacking the capability answer `SOURCE_CAPABILITY_UNSUPPORTED`, which
/// lands in `source_errors` like any other per-source failure instead of
/// failing the request.
pub async fn aggregate_collections(
    sources: Vec<Arc<dyn MusicSource>>,
    query: SearchQuery,
    kind: CollectionKind,
    timeout: Duration,
) -> Result<AggregatedCollections, SourceError> {
    if query.query.trim().is_empty() {
        return Err(invalid("搜索关键词不能为空"));
    }
    if query.page == 0 || query.page_size == 0 {
        return Err(invalid("分页参数无效"));
    }
    if sources.is_empty() {
        return Err(invalid("至少选择一个音乐源"));
    }

    let queried_sources = sources.len();
    let mut tasks = JoinSet::new();
    for source in sources {
        let mut per_source = query.clone();
        per_source.sources = vec![source.id().to_owned()];
        tasks.spawn(async move {
            let id = source.id().to_owned();
            let listing = async {
                match kind {
                    CollectionKind::Playlist => source.search_playlists(per_source).await,
                    CollectionKind::Album => source.search_albums(per_source).await,
                }
            };
            (id, tokio::time::timeout(timeout, listing).await)
        });
    }

    let mut items = Vec::new();
    let mut source_errors = Vec::new();
    let mut total = 0_u64;
    let mut responded_sources = 0_usize;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((_, Ok(Ok(page)))) => {
                responded_sources += 1;
                total = total.saturating_add(page.total.unwrap_or(page.items.len() as u64));
                items.extend(page.items);
            }
            Ok((id, Ok(Err(error)))) => {
                tracing::warn!(
                    event = "collection_search_failed",
                    source = %id,
                    kind = kind.as_str(),
                    code = %error.code
                );
                source_errors.push(SourceFailure::from_error(id, error));
            }
            Ok((id, Err(_elapsed))) => {
                tracing::warn!(event = "collection_search_timeout", source = %id);
                source_errors.push(SourceFailure::timeout(id, timeout));
            }
            Err(join_error) => {
                tracing::warn!(event = "collection_search_panicked", %join_error);
                source_errors.push(SourceFailure::panicked("unknown".to_owned()));
            }
        }
    }

    if responded_sources == 0 {
        return Err(all_failed(&source_errors));
    }

    Ok(AggregatedCollections {
        items,
        total: Some(total),
        page: query.page,
        page_size: query.page_size,
        source_errors,
        queried_sources,
        responded_sources,
    })
}

#[cfg(test)]
mod tests {
    use super::{aggregate_collections, aggregate_search, CollectionKind, SOURCE_TIMEOUT};
    use crate::domain::collection::{Collection, CollectionType};
    use crate::domain::source::{
        Artist, CollectionPage, MusicSource, SearchQuery, SearchResult, Song, SongRef,
        SourceCapabilities, SourceError,
    };
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::time::Duration;

    /// Scripted source: returns fixed rows, an error, or stalls past the
    /// per-source deadline.
    struct FakeSource {
        id: &'static str,
        songs: Vec<Song>,
        error: Option<SourceError>,
        delay: Option<Duration>,
        playlists: Option<Vec<Collection>>,
    }

    impl FakeSource {
        fn with_songs(id: &'static str, songs: Vec<Song>) -> Arc<dyn MusicSource> {
            Arc::new(Self {
                id,
                songs,
                error: None,
                delay: None,
                playlists: None,
            })
        }

        fn failing(id: &'static str, code: &str) -> Arc<dyn MusicSource> {
            Arc::new(Self {
                id,
                songs: Vec::new(),
                error: Some(SourceError {
                    code: code.to_owned(),
                    message: format!("{id} 出错"),
                    retryable: true,
                }),
                delay: None,
                playlists: None,
            })
        }

        fn stalling(id: &'static str, delay: Duration) -> Arc<dyn MusicSource> {
            Arc::new(Self {
                id,
                songs: Vec::new(),
                error: None,
                delay: Some(delay),
                playlists: None,
            })
        }

        fn with_playlists(id: &'static str, playlists: Vec<Collection>) -> Arc<dyn MusicSource> {
            Arc::new(Self {
                id,
                songs: Vec::new(),
                error: None,
                delay: None,
                playlists: Some(playlists),
            })
        }
    }

    #[async_trait]
    impl MusicSource for FakeSource {
        fn id(&self) -> &str {
            self.id
        }

        fn capabilities(&self) -> SourceCapabilities {
            SourceCapabilities {
                song_search: true,
                playlist_search: self.playlists.is_some(),
                album_search: false,
                lyrics: false,
                cover: false,
                cookie: false,
                qr_login: false,
                qualities: vec![],
            }
        }

        async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            Ok(SearchResult {
                total: Some(self.songs.len() as u64),
                items: self.songs.clone(),
                page: query.page,
                page_size: query.page_size,
            })
        }

        async fn search_playlists(
            &self,
            query: SearchQuery,
        ) -> Result<CollectionPage, SourceError> {
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            match &self.playlists {
                Some(items) => Ok(CollectionPage {
                    total: Some(items.len() as u64),
                    items: items.clone(),
                    page: query.page,
                    page_size: query.page_size,
                }),
                None => Err(crate::domain::source::unsupported(
                    self.id,
                    "playlist search",
                )),
            }
        }
    }

    fn song(source: &str, id: &str, title: &str, artist: &str, duration_ms: u64) -> Song {
        Song {
            reference: SongRef {
                source: source.to_owned(),
                scope: "song".to_owned(),
                site_id: id.to_owned(),
            },
            title: title.to_owned(),
            artists: vec![Artist {
                id: None,
                name: artist.to_owned(),
            }],
            album: None,
            duration_ms: Some(duration_ms),
            cover_url: None,
        }
    }

    fn query(keyword: &str) -> SearchQuery {
        SearchQuery {
            query: keyword.to_owned(),
            page: 1,
            page_size: 20,
            sources: Vec::new(),
        }
    }

    #[tokio::test]
    async fn merges_results_from_every_source() {
        let sources = vec![
            FakeSource::with_songs(
                "alpha",
                vec![song("alpha", "1", "Blue Sky", "Nova", 200_000)],
            ),
            FakeSource::with_songs(
                "beta",
                vec![song("beta", "2", "Green Field", "Nova", 240_000)],
            ),
        ];
        let result = aggregate_search(sources, query("Blue Sky"), SOURCE_TIMEOUT)
            .await
            .expect("both sources answer");
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.responded_sources, 2);
        assert_eq!(result.queried_sources, 2);
        assert!(result.source_errors.is_empty());
        assert_eq!(result.total, Some(2));
    }

    #[tokio::test]
    async fn ranking_puts_the_exact_title_and_artist_match_first() {
        let sources = vec![FakeSource::with_songs(
            "alpha",
            vec![
                song("alpha", "1", "Blue Sky", "Someone Else", 200_000),
                song("alpha", "2", "Blue Skyline", "Nova", 201_000),
                song("alpha", "3", "Blue Sky", "Nova", 200_000),
            ],
        )];
        let result = aggregate_search(sources, query("Blue Sky Nova"), SOURCE_TIMEOUT)
            .await
            .expect("source answers");
        let best = &result.items[0].song;
        assert_eq!(best.reference.site_id, "3");
        assert!(
            result.items[0].score > result.items[1].score,
            "the exact match must outrank the partial ones"
        );
    }

    #[tokio::test]
    async fn one_failing_source_still_yields_results_and_records_the_error() {
        let sources = vec![
            FakeSource::with_songs(
                "alpha",
                vec![song("alpha", "1", "Blue Sky", "Nova", 200_000)],
            ),
            FakeSource::failing("beta", "SOURCE_HTTP_ERROR"),
        ];
        let result = aggregate_search(sources, query("Blue Sky"), SOURCE_TIMEOUT)
            .await
            .expect("a partial failure must not fail the search");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.responded_sources, 1);
        assert_eq!(result.queried_sources, 2);
        let failure = &result.source_errors[0];
        assert_eq!(failure.source, "beta");
        assert_eq!(failure.code, "SOURCE_HTTP_ERROR");
    }

    #[tokio::test]
    async fn all_sources_failing_is_an_error() {
        let sources = vec![
            FakeSource::failing("alpha", "SOURCE_HTTP_ERROR"),
            FakeSource::failing("beta", "SOURCE_HTTP_ERROR"),
        ];
        let error = aggregate_search(sources, query("Blue Sky"), SOURCE_TIMEOUT)
            .await
            .expect_err("no source responded");
        assert_eq!(error.code, "SEARCH_ALL_SOURCES_FAILED");
    }

    #[tokio::test]
    async fn a_slow_source_hits_the_timeout_without_failing_the_request() {
        let sources = vec![
            FakeSource::with_songs(
                "alpha",
                vec![song("alpha", "1", "Blue Sky", "Nova", 200_000)],
            ),
            FakeSource::stalling("beta", Duration::from_secs(30)),
        ];
        let result = aggregate_search(sources, query("Blue Sky"), Duration::from_millis(50))
            .await
            .expect("the healthy source still answers");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.responded_sources, 1);
        let failure = &result.source_errors[0];
        assert_eq!(failure.source, "beta");
        assert_eq!(failure.code, "SOURCE_TIMEOUT");
        assert!(failure.retryable);
    }

    #[tokio::test]
    async fn cross_source_duplicates_collapse_and_record_alternatives() {
        let sources = vec![
            FakeSource::with_songs(
                "alpha",
                vec![song("alpha", "1", "Blue Sky", "Nova", 200_000)],
            ),
            // Same track, punctuation and casing differ, duration within 10s.
            FakeSource::with_songs(
                "beta",
                vec![song("beta", "9", "blue sky!", "nova", 203_000)],
            ),
            FakeSource::with_songs(
                "gamma",
                vec![song("gamma", "7", "Blue Sky", "Nova", 200_500)],
            ),
        ];
        let result = aggregate_search(sources, query("Blue Sky Nova"), SOURCE_TIMEOUT)
            .await
            .expect("all sources answer");
        assert_eq!(
            result.items.len(),
            1,
            "the same track must collapse to one row"
        );
        let row = &result.items[0];
        assert_eq!(row.alternatives.len(), 2);
        let mut alternates: Vec<&str> = row
            .alternatives
            .iter()
            .map(|item| item.source.as_str())
            .collect();
        alternates.sort();
        let kept = row.song.reference.source.as_str();
        assert!(
            !alternates.contains(&kept),
            "the kept row is not its own alternative"
        );
        assert_eq!(alternates.len(), 2);
        assert_eq!(result.total, Some(3), "the upstream total stays pre-dedupe");
    }

    #[tokio::test]
    async fn same_title_with_a_distant_duration_stays_separate() {
        let sources = vec![
            FakeSource::with_songs(
                "alpha",
                vec![song("alpha", "1", "Blue Sky", "Nova", 200_000)],
            ),
            // A 9-minute live cut is a different recording, not a duplicate.
            FakeSource::with_songs("beta", vec![song("beta", "2", "Blue Sky", "Nova", 540_000)]),
        ];
        let result = aggregate_search(sources, query("Blue Sky"), SOURCE_TIMEOUT)
            .await
            .expect("both sources answer");
        assert_eq!(result.items.len(), 2);
        assert!(result.items.iter().all(|row| row.alternatives.is_empty()));
    }

    #[tokio::test]
    async fn artist_order_does_not_prevent_dedupe() {
        let mut left = song("alpha", "1", "Duet", "Ann", 200_000);
        left.artists.push(Artist {
            id: None,
            name: "Bob".into(),
        });
        let mut right = song("beta", "2", "Duet", "Bob", 200_000);
        right.artists.push(Artist {
            id: None,
            name: "Ann".into(),
        });
        let sources = vec![
            FakeSource::with_songs("alpha", vec![left]),
            FakeSource::with_songs("beta", vec![right]),
        ];
        let result = aggregate_search(sources, query("Duet"), SOURCE_TIMEOUT)
            .await
            .expect("both sources answer");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].alternatives.len(), 1);
    }

    #[tokio::test]
    async fn fan_out_runs_concurrently_rather_than_in_sequence() {
        // Four sources each sleeping 150ms: serial execution needs 600ms, so a
        // 400ms ceiling only passes when the fan-out is genuinely concurrent.
        let delay = Duration::from_millis(150);
        let sources: Vec<_> = ["alpha", "beta", "gamma", "delta"]
            .into_iter()
            .map(|id| {
                Arc::new(FakeSource {
                    id,
                    songs: vec![song(id, "1", "Blue Sky", "Nova", 200_000)],
                    error: None,
                    delay: Some(delay),
                    playlists: None,
                }) as Arc<dyn MusicSource>
            })
            .collect();
        let started = std::time::Instant::now();
        let result = aggregate_search(sources, query("Blue Sky"), SOURCE_TIMEOUT)
            .await
            .expect("every source answers");
        let elapsed = started.elapsed();
        assert_eq!(result.responded_sources, 4);
        assert!(
            elapsed < Duration::from_millis(400),
            "fan-out took {elapsed:?}, which suggests sequential queries"
        );
    }

    #[tokio::test]
    async fn empty_keyword_is_rejected_before_any_source_call() {
        let sources = vec![FakeSource::failing("alpha", "SOURCE_HTTP_ERROR")];
        let error = aggregate_search(sources, query("   "), SOURCE_TIMEOUT)
            .await
            .expect_err("a blank keyword is invalid");
        assert_eq!(error.code, "INVALID_ARGUMENT");
    }

    #[tokio::test]
    async fn no_selected_source_is_rejected() {
        let error = aggregate_search(Vec::new(), query("Blue Sky"), SOURCE_TIMEOUT)
            .await
            .expect_err("an empty source list is invalid");
        assert_eq!(error.code, "INVALID_ARGUMENT");
    }

    fn playlist(source: &str, id: &str, name: &str) -> Collection {
        Collection {
            id: id.to_owned(),
            source: source.to_owned(),
            content_type: CollectionType::Playlist,
            name: name.to_owned(),
            description: None,
            creator: None,
            cover_url: None,
            track_count: Some(10),
        }
    }

    #[tokio::test]
    async fn collection_search_merges_and_reports_unsupported_sources() {
        let sources = vec![
            FakeSource::with_playlists("alpha", vec![playlist("alpha", "p1", "Morning Mix")]),
            // No playlist capability: reported, not fatal.
            FakeSource::with_songs("beta", vec![]),
        ];
        let result = aggregate_collections(
            sources,
            query("Morning"),
            CollectionKind::Playlist,
            SOURCE_TIMEOUT,
        )
        .await
        .expect("one source supports playlists");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.responded_sources, 1);
        assert_eq!(
            result.source_errors[0].code,
            "SOURCE_CAPABILITY_UNSUPPORTED"
        );
    }

    #[tokio::test]
    async fn album_search_unsupported_everywhere_is_an_error() {
        let sources = vec![FakeSource::with_songs("alpha", vec![])];
        let error = aggregate_collections(
            sources,
            query("Morning"),
            CollectionKind::Album,
            SOURCE_TIMEOUT,
        )
        .await
        .expect_err("no source supports album search");
        assert_eq!(error.code, "SOURCE_CAPABILITY_UNSUPPORTED");
    }
}
