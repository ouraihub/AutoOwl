//! Automatic source switching.
//!
//! Probe the primary source, and when it cannot serve the audio, search the
//! other enabled sources for the same recording and return the first candidate
//! that actually plays.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use crate::domain::media::PlayableResource;
use crate::domain::similarity::{duration_close, song_similarity};
use crate::domain::source::{MusicSource, SearchQuery, Song, SongRef, SourceError};
use crate::infrastructure::http::{source_headers, ByteFetcher};

use super::search::{artist_names, SOURCE_TIMEOUT};

/// Playability probe deadline.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Minimum [`song_similarity`] for a cross-source candidate.
///
/// Deliberately strict: a wrong-but-playable track is worse for the listener
/// than reporting that no alternative was found.
pub const MATCH_THRESHOLD: f64 = 0.6;

/// Concurrent probes while hunting for an alternative.
pub const PROBE_CONCURRENCY: usize = 5;

/// Sources whose stream URLs cannot be probed with a plain ranged GET.
///
/// Soda serves encrypted bodies, fivesing and local are not ranged HTTP
/// resources. Probing them would look like a failure, so they are reported as
/// unprobeable and trusted instead.
pub const UNPROBEABLE_SOURCES: [&str; 4] = ["soda", "fivesing", "local", "local-file"];

pub fn is_probeable(source: &str) -> bool {
    !UNPROBEABLE_SOURCES.contains(&source)
}

/// Outcome of a playability probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Playability {
    /// The server answered 200/206 for `bytes=0-1`.
    Playable,
    /// The URL resolved but the server refused to serve bytes.
    Unplayable,
    /// The source cannot be probed this way; not a negative result.
    Unprobeable,
}

/// A probe result plus the resource it was performed against.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayabilityReport {
    pub source: String,
    pub playability: Playability,
    /// Present whenever the URL resolved, even if it then failed to serve bytes.
    pub resource: Option<PlayableResource>,
    /// Why the probe did not succeed; `None` when playable.
    pub detail: Option<String>,
}

impl PlayabilityReport {
    pub fn is_usable(&self) -> bool {
        matches!(
            self.playability,
            Playability::Playable | Playability::Unprobeable
        )
    }
}

/// Playback served after an automatic switch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPlayback {
    pub resource: PlayableResource,
    /// The source that actually served the audio.
    pub served_by: String,
    /// The source originally requested.
    pub requested: String,
    pub switched: bool,
    pub playability: Playability,
    /// Sources tried and rejected before this one, for UI diagnostics.
    pub attempts: Vec<PlayabilityReport>,
}

/// Resolves a stream URL and probes it with `Range: bytes=0-1`.
///
/// Resolution failure is an error; a resolved-but-refusing URL is
/// [`Playability::Unplayable`], because the caller distinguishes "no URL at all"
/// from "URL that will not play" when deciding whether to switch source.
pub async fn validate_playable(
    source: &Arc<dyn MusicSource>,
    fetcher: &dyn ByteFetcher,
    cookie: &str,
    site_id: &str,
) -> Result<PlayabilityReport, SourceError> {
    let id = source.id().to_owned();
    let resource = source
        .resolve_song(SongRef {
            source: id.clone(),
            scope: "song".to_owned(),
            site_id: site_id.to_owned(),
        })
        .await?;

    if !is_probeable(&id) {
        return Ok(PlayabilityReport {
            detail: Some(format!("{id} 不支持可播性探测，已按可播处理")),
            source: id,
            playability: Playability::Unprobeable,
            resource: Some(resource),
        });
    }

    let headers = source_headers(&id, cookie);
    let probe = fetcher
        .fetch_range(&resource.url, &headers, 0, Some(1), PROBE_TIMEOUT)
        .await;
    let (playability, detail) = match probe {
        // 200 is accepted as well as 206: sources that ignore Range
        // still stream fine.
        Ok(response) if response.is_success() => (Playability::Playable, None),
        Ok(response) => (
            Playability::Unplayable,
            Some(format!("音源返回状态码 {}", response.status)),
        ),
        Err(error) => (Playability::Unplayable, Some(error.to_string())),
    };
    Ok(PlayabilityReport {
        source: id,
        playability,
        resource: Some(resource),
        detail,
    })
}

/// A cross-source candidate that passed the similarity and duration gates.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub song: Song,
    pub score: f64,
}

/// Searches the other enabled sources for the same recording, keeping only
/// candidates that clear both the similarity and duration gates, best first.
pub async fn find_candidates(
    sources: Vec<Arc<dyn MusicSource>>,
    song: &Song,
    timeout: Duration,
) -> Vec<Candidate> {
    let title = song.title.trim();
    if title.is_empty() {
        return Vec::new();
    }
    let artists = artist_names(song);
    let keyword = if artists.is_empty() {
        title.to_owned()
    } else {
        format!("{title} {artists}")
    };
    let excluded = song.reference.source.clone();

    let mut tasks = JoinSet::new();
    for candidate_source in sources {
        if candidate_source.id() == excluded {
            continue;
        }
        let query = SearchQuery {
            query: keyword.clone(),
            page: 1,
            page_size: 20,
            sources: vec![candidate_source.id().to_owned()],
        };
        tasks.spawn(async move {
            let id = candidate_source.id().to_owned();
            let outcome = tokio::time::timeout(timeout, candidate_source.search_songs(query)).await;
            (id, outcome)
        });
    }

    let mut candidates = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let Ok((id, Ok(Ok(result)))) = joined else {
            continue;
        };
        for found in result.items {
            let score = song_similarity(title, &artists, &found.title, &artist_names(&found));
            if score < MATCH_THRESHOLD {
                continue;
            }
            if !duration_close(
                song.duration_ms.unwrap_or(0),
                found.duration_ms.unwrap_or(0),
            ) {
                tracing::debug!(
                    event = "switch_candidate_duration_rejected",
                    source = %id,
                    title = %found.title
                );
                continue;
            }
            candidates.push(Candidate { song: found, score });
        }
    }
    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.song.reference.source.cmp(&right.song.reference.source))
    });
    candidates
}

/// Probe outcome of the alternative hunt.
pub struct AlternativeSearch {
    /// Best-ranked candidate that actually plays.
    pub winner: Option<PlayabilityReport>,
    /// Candidates probed and rejected, best-ranked first.
    pub attempts: Vec<PlayabilityReport>,
}

/// Finds a playable replacement for `song` on another source.
///
/// Candidates are gated on similarity *and* duration, then probed in
/// best-ranked batches of [`PROBE_CONCURRENCY`]. Batching keeps the probes
/// concurrent while still preferring the better match: whichever candidate in a
/// batch ranks highest wins, rather than whichever server answered fastest.
pub async fn find_alternative(
    sources: Vec<Arc<dyn MusicSource>>,
    fetcher: Arc<dyn ByteFetcher>,
    cookies: Arc<crate::infrastructure::credentials::CookieStore>,
    song: &Song,
) -> AlternativeSearch {
    let candidates = find_candidates(sources.clone(), song, SOURCE_TIMEOUT).await;
    let mut attempts = Vec::new();
    for batch in candidates.chunks(PROBE_CONCURRENCY) {
        let mut tasks = JoinSet::new();
        for (rank, candidate) in batch.iter().enumerate() {
            let Some(source) = sources
                .iter()
                .find(|item| item.id() == candidate.song.reference.source)
                .cloned()
            else {
                continue;
            };
            let fetcher = Arc::clone(&fetcher);
            let cookie = cookies.get(&candidate.song.reference.source);
            let site_id = candidate.song.reference.site_id.clone();
            tasks.spawn(async move {
                let report = validate_playable(&source, fetcher.as_ref(), &cookie, &site_id).await;
                (rank, report)
            });
        }

        let mut probed: Vec<(usize, PlayabilityReport)> = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((rank, Ok(report))) => probed.push((rank, report)),
                Ok((rank, Err(error))) => probed.push((
                    rank,
                    PlayabilityReport {
                        source: batch
                            .get(rank)
                            .map(|item| item.song.reference.source.clone())
                            .unwrap_or_default(),
                        playability: Playability::Unplayable,
                        resource: None,
                        detail: Some(error.message),
                    },
                )),
                Err(join_error) => {
                    tracing::warn!(event = "switch_probe_panicked", %join_error);
                }
            }
        }
        probed.sort_by_key(|(rank, _)| *rank);

        let mut winner = None;
        for (_, report) in probed {
            if winner.is_none() && report.is_usable() {
                winner = Some(report);
            } else {
                attempts.push(report);
            }
        }
        if winner.is_some() {
            return AlternativeSearch { winner, attempts };
        }
    }
    AlternativeSearch {
        winner: None,
        attempts,
    }
}

/// Resolves playback for `song`, switching source when the primary cannot serve
/// the audio and `auto_switch` is enabled.
///
/// When switching is off, or no alternative plays, the primary error surfaces —
/// silently returning a URL known to be dead would only fail in the WebView.
pub async fn resolve_with_fallback(
    sources: Vec<Arc<dyn MusicSource>>,
    fetcher: Arc<dyn ByteFetcher>,
    cookies: Arc<crate::infrastructure::credentials::CookieStore>,
    song: &Song,
    auto_switch: bool,
) -> Result<ResolvedPlayback, SourceError> {
    let requested = song.reference.source.clone();
    let primary = sources
        .iter()
        .find(|item| item.id() == requested)
        .cloned()
        .ok_or_else(|| SourceError {
            code: "SOURCE_NOT_FOUND".to_owned(),
            message: format!("音乐源不存在：{requested}"),
            retryable: false,
        })?;

    let cookie = cookies.get(&requested);
    let primary_report =
        validate_playable(&primary, fetcher.as_ref(), &cookie, &song.reference.site_id).await;

    let mut attempts = Vec::new();
    match &primary_report {
        Ok(report) if report.is_usable() => {
            if let Some(resource) = report.resource.clone() {
                return Ok(ResolvedPlayback {
                    resource,
                    served_by: requested.clone(),
                    requested,
                    switched: false,
                    playability: report.playability,
                    attempts,
                });
            }
        }
        Ok(report) => attempts.push(report.clone()),
        Err(error) => attempts.push(PlayabilityReport {
            source: requested.clone(),
            playability: Playability::Unplayable,
            resource: None,
            detail: Some(error.message.clone()),
        }),
    }

    if !auto_switch {
        tracing::debug!(event = "switch_disabled", source = %requested);
        return Err(primary_failure(primary_report, &requested));
    }

    tracing::info!(event = "switch_started", source = %requested, title = %song.title);
    let search = find_alternative(sources, fetcher, cookies, song).await;
    attempts.extend(search.attempts);
    match search.winner {
        Some(report) => {
            let served_by = report.source.clone();
            let Some(resource) = report.resource.clone() else {
                return Err(primary_failure(primary_report, &requested));
            };
            tracing::info!(event = "switch_succeeded", from = %requested, to = %served_by);
            Ok(ResolvedPlayback {
                resource,
                served_by,
                requested,
                switched: true,
                playability: report.playability,
                attempts,
            })
        }
        None => {
            tracing::warn!(event = "switch_exhausted", source = %requested);
            Err(SourceError {
                code: "PLAYBACK_NO_PLAYABLE_SOURCE".to_owned(),
                message: format!("{} 无法播放，且没有找到可用的替代音源", song.title),
                retryable: true,
            })
        }
    }
}

/// Turns the primary probe outcome into the error the caller sees.
fn primary_failure(report: Result<PlayabilityReport, SourceError>, source: &str) -> SourceError {
    match report {
        // Resolution succeeded but the stream refuses to serve bytes.
        Ok(report) => SourceError {
            code: "PLAYBACK_URL_UNPLAYABLE".to_owned(),
            message: report
                .detail
                .unwrap_or_else(|| format!("{source} 返回的播放地址不可用")),
            retryable: true,
        },
        Err(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        find_candidates, is_probeable, resolve_with_fallback, validate_playable, Playability,
        MATCH_THRESHOLD, SOURCE_TIMEOUT,
    };
    use crate::domain::media::PlayableResource;
    use crate::domain::source::{
        Artist, MusicSource, SearchQuery, SearchResult, Song, SongRef, SourceCapabilities,
        SourceError,
    };
    use crate::infrastructure::credentials::CookieStore;
    use crate::infrastructure::http::{ByteFetcher, BytesResponse, HttpError};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Source that resolves every song to `https://<id>.test/<site_id>` and can
    /// be scripted to fail resolution or return search rows.
    struct FakeSource {
        id: &'static str,
        songs: Vec<Song>,
        resolve_error: Option<SourceError>,
    }

    impl FakeSource {
        fn resolving(id: &'static str, songs: Vec<Song>) -> Arc<dyn MusicSource> {
            Arc::new(Self {
                id,
                songs,
                resolve_error: None,
            })
        }

        fn unresolvable(id: &'static str) -> Arc<dyn MusicSource> {
            Arc::new(Self {
                id,
                songs: Vec::new(),
                resolve_error: Some(SourceError {
                    code: "SOURCE_NO_COPYRIGHT".to_owned(),
                    message: format!("{id} 无版权"),
                    retryable: false,
                }),
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
                playlist_search: false,
                album_search: false,
                lyrics: false,
                cover: false,
                cookie: false,
                qr_login: false,
                qualities: vec![],
            }
        }

        async fn search_songs(&self, query: SearchQuery) -> Result<SearchResult, SourceError> {
            Ok(SearchResult {
                total: Some(self.songs.len() as u64),
                items: self.songs.clone(),
                page: query.page,
                page_size: query.page_size,
            })
        }

        async fn resolve_song(&self, song: SongRef) -> Result<PlayableResource, SourceError> {
            if let Some(error) = &self.resolve_error {
                return Err(error.clone());
            }
            Ok(PlayableResource {
                url: format!("https://{}.test/{}", self.id, song.site_id),
                song,
                quality: "standard".to_owned(),
                duration_ms: Some(200_000),
                expires_at: None,
            })
        }
    }

    /// Fetcher answering per-URL-substring status codes, recording every probe.
    struct FakeFetcher {
        statuses: HashMap<&'static str, u16>,
        default_status: u16,
        probed: Mutex<Vec<String>>,
        concurrent: AtomicUsize,
        peak_concurrent: AtomicUsize,
        delay: Option<Duration>,
    }

    impl FakeFetcher {
        fn new(statuses: &[(&'static str, u16)], default_status: u16) -> Arc<Self> {
            Arc::new(Self {
                statuses: statuses.iter().copied().collect(),
                default_status,
                probed: Mutex::new(Vec::new()),
                concurrent: AtomicUsize::new(0),
                peak_concurrent: AtomicUsize::new(0),
                delay: None,
            })
        }

        fn slow(
            statuses: &[(&'static str, u16)],
            default_status: u16,
            delay: Duration,
        ) -> Arc<Self> {
            Arc::new(Self {
                statuses: statuses.iter().copied().collect(),
                default_status,
                probed: Mutex::new(Vec::new()),
                concurrent: AtomicUsize::new(0),
                peak_concurrent: AtomicUsize::new(0),
                delay: Some(delay),
            })
        }

        fn probes(&self) -> Vec<String> {
            self.probed
                .lock()
                .map(|log| log.clone())
                .unwrap_or_default()
        }

        fn status_for(&self, url: &str) -> u16 {
            self.statuses
                .iter()
                .find(|(needle, _)| url.contains(*needle))
                .map(|(_, status)| *status)
                .unwrap_or(self.default_status)
        }
    }

    #[async_trait]
    impl ByteFetcher for FakeFetcher {
        async fn fetch_range(
            &self,
            url: &str,
            _headers: &[(&str, String)],
            start: u64,
            end: Option<u64>,
            _timeout: Duration,
        ) -> Result<BytesResponse, HttpError> {
            // The probe contract is exactly `bytes=0-1`.
            assert_eq!(start, 0);
            assert_eq!(end, Some(1));
            let live = self.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_concurrent.fetch_max(live, Ordering::SeqCst);
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            self.concurrent.fetch_sub(1, Ordering::SeqCst);
            if let Ok(mut log) = self.probed.lock() {
                log.push(url.to_owned());
            }
            let status = self.status_for(url);
            if status == 0 {
                return Err(HttpError::Failed("连接被拒绝".to_owned()));
            }
            Ok(BytesResponse {
                status,
                body: vec![0, 1],
                total: Some(1024),
                content_type: Some("audio/mpeg".to_owned()),
            })
        }

        async fn fetch_all(
            &self,
            url: &str,
            headers: &[(&str, String)],
            timeout: Duration,
        ) -> Result<BytesResponse, HttpError> {
            self.fetch_range(url, headers, 0, Some(1), timeout).await
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

    fn cookies() -> Arc<CookieStore> {
        Arc::new(CookieStore::default())
    }

    #[tokio::test]
    async fn probe_treats_200_and_206_as_playable() {
        let fetcher = FakeFetcher::new(&[("alpha.test", 206), ("beta.test", 200)], 404);
        for id in ["alpha", "beta"] {
            let source = FakeSource::resolving(id, Vec::new());
            let report = validate_playable(&source, fetcher.as_ref(), "", "1")
                .await
                .expect("resolution succeeds");
            assert_eq!(report.playability, Playability::Playable);
            assert!(report.resource.is_some());
        }
    }

    #[tokio::test]
    async fn probe_marks_a_refusing_url_unplayable_and_keeps_the_resource() {
        let fetcher = FakeFetcher::new(&[], 403);
        let source = FakeSource::resolving("alpha", Vec::new());
        let report = validate_playable(&source, fetcher.as_ref(), "", "1")
            .await
            .expect("the URL still resolves");
        assert_eq!(report.playability, Playability::Unplayable);
        assert!(
            report.resource.is_some(),
            "the caller needs the URL for diagnostics"
        );
        assert!(report.detail.is_some());
    }

    #[tokio::test]
    async fn unprobeable_sources_are_reported_as_such_not_as_failures() {
        // A 403 default would fail any real probe; soda must never be probed.
        let fetcher = FakeFetcher::new(&[], 403);
        for id in ["soda", "fivesing", "local"] {
            assert!(!is_probeable(id));
            let source = FakeSource::resolving(id, Vec::new());
            let report = validate_playable(&source, fetcher.as_ref(), "", "1")
                .await
                .expect("resolution succeeds");
            assert_eq!(
                report.playability,
                Playability::Unprobeable,
                "{id} must not be probed"
            );
            assert!(report.is_usable());
        }
        assert!(
            fetcher.probes().is_empty(),
            "unprobeable sources must not issue a range request"
        );
    }

    #[tokio::test]
    async fn candidates_below_the_similarity_threshold_are_rejected() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        let sources = vec![FakeSource::resolving(
            "beta",
            vec![song(
                "beta",
                "9",
                "Completely Different Song",
                "Other Band",
                200_000,
            )],
        )];
        let candidates = find_candidates(sources, &wanted, SOURCE_TIMEOUT).await;
        assert!(
            candidates.is_empty(),
            "an unrelated title must not pass the similarity gate"
        );
    }

    #[tokio::test]
    async fn candidates_failing_the_duration_gate_are_rejected() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        // Same title and artist, but a 9-minute cut: a different recording.
        let sources = vec![FakeSource::resolving(
            "beta",
            vec![song("beta", "9", "Blue Sky", "Nova", 540_000)],
        )];
        let candidates = find_candidates(sources, &wanted, SOURCE_TIMEOUT).await;
        assert!(
            candidates.is_empty(),
            "a distant duration must not pass the duration gate"
        );
    }

    #[tokio::test]
    async fn candidates_exclude_the_failing_source_and_rank_best_first() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        let sources = vec![
            FakeSource::resolving(
                "alpha",
                vec![song("alpha", "1", "Blue Sky", "Nova", 200_000)],
            ),
            FakeSource::resolving(
                "beta",
                vec![song("beta", "2", "Blue Sky Remix", "Nova", 202_000)],
            ),
            FakeSource::resolving(
                "gamma",
                vec![song("gamma", "3", "Blue Sky", "Nova", 200_000)],
            ),
        ];
        let candidates = find_candidates(sources, &wanted, SOURCE_TIMEOUT).await;
        assert!(
            candidates
                .iter()
                .all(|item| item.song.reference.source != "alpha"),
            "the failing source must not be its own alternative"
        );
        assert_eq!(candidates[0].song.reference.source, "gamma");
        assert!(candidates[0].score >= MATCH_THRESHOLD);
    }

    #[tokio::test]
    async fn unplayable_primary_falls_back_to_a_validated_alternative() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        let sources = vec![
            FakeSource::resolving("alpha", vec![wanted.clone()]),
            FakeSource::resolving("beta", vec![song("beta", "2", "Blue Sky", "Nova", 200_000)]),
        ];
        // The primary refuses bytes; the alternative serves them.
        let fetcher = FakeFetcher::new(&[("alpha.test", 403), ("beta.test", 206)], 404);
        let resolved = resolve_with_fallback(sources, fetcher, cookies(), &wanted, true)
            .await
            .expect("the alternative plays");
        assert!(resolved.switched);
        assert_eq!(resolved.requested, "alpha");
        assert_eq!(resolved.served_by, "beta");
        assert_eq!(resolved.playability, Playability::Playable);
        assert!(resolved.resource.url.contains("beta.test"));
        assert!(
            resolved.attempts.iter().any(|item| item.source == "alpha"),
            "the rejected primary is reported"
        );
    }

    #[tokio::test]
    async fn a_playable_primary_is_served_without_switching() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        let sources = vec![
            FakeSource::resolving("alpha", vec![wanted.clone()]),
            FakeSource::resolving("beta", vec![song("beta", "2", "Blue Sky", "Nova", 200_000)]),
        ];
        let fetcher = FakeFetcher::new(&[("alpha.test", 206)], 403);
        let probe_fetcher = Arc::clone(&fetcher) as Arc<dyn ByteFetcher>;
        let resolved = resolve_with_fallback(sources, probe_fetcher, cookies(), &wanted, true)
            .await
            .expect("the primary plays");
        assert!(!resolved.switched);
        assert_eq!(resolved.served_by, "alpha");
        assert_eq!(
            fetcher.probes().len(),
            1,
            "a healthy primary must not trigger candidate probes"
        );
    }

    #[tokio::test]
    async fn fallback_is_skipped_when_the_setting_is_off() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        let sources = vec![
            FakeSource::resolving("alpha", vec![wanted.clone()]),
            FakeSource::resolving("beta", vec![song("beta", "2", "Blue Sky", "Nova", 200_000)]),
        ];
        let fetcher = FakeFetcher::new(&[("alpha.test", 403), ("beta.test", 206)], 404);
        let probe_fetcher = Arc::clone(&fetcher) as Arc<dyn ByteFetcher>;
        let error = resolve_with_fallback(sources, probe_fetcher, cookies(), &wanted, false)
            .await
            .expect_err("switching is disabled, so the primary failure surfaces");
        assert_eq!(error.code, "PLAYBACK_URL_UNPLAYABLE");
        assert!(
            fetcher
                .probes()
                .iter()
                .all(|url| url.contains("alpha.test")),
            "no alternative may be probed when switching is off"
        );
    }

    #[tokio::test]
    async fn an_unresolvable_primary_still_falls_back() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        let sources = vec![
            // Resolution itself fails, not just the probe.
            FakeSource::unresolvable("alpha"),
            FakeSource::resolving("beta", vec![song("beta", "2", "Blue Sky", "Nova", 200_000)]),
        ];
        let fetcher = FakeFetcher::new(&[("beta.test", 206)], 404);
        let resolved = resolve_with_fallback(sources, fetcher, cookies(), &wanted, true)
            .await
            .expect("the alternative plays");
        assert!(resolved.switched);
        assert_eq!(resolved.served_by, "beta");
    }

    #[tokio::test]
    async fn an_unresolvable_primary_with_switching_off_surfaces_the_source_error() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        let sources = vec![FakeSource::unresolvable("alpha")];
        let fetcher = FakeFetcher::new(&[], 404);
        let error = resolve_with_fallback(sources, fetcher, cookies(), &wanted, false)
            .await
            .expect_err("resolution failed");
        assert_eq!(error.code, "SOURCE_NO_COPYRIGHT");
    }

    #[tokio::test]
    async fn no_playable_alternative_reports_exhaustion() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        let sources = vec![
            FakeSource::resolving("alpha", vec![wanted.clone()]),
            FakeSource::resolving("beta", vec![song("beta", "2", "Blue Sky", "Nova", 200_000)]),
        ];
        // Nothing serves bytes anywhere.
        let fetcher = FakeFetcher::new(&[], 403);
        let error = resolve_with_fallback(sources, fetcher, cookies(), &wanted, true)
            .await
            .expect_err("no source can play");
        assert_eq!(error.code, "PLAYBACK_NO_PLAYABLE_SOURCE");
        assert!(error.retryable);
    }

    #[tokio::test]
    async fn an_unprobeable_alternative_is_accepted_as_the_switch_target() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        let sources = vec![
            FakeSource::resolving("alpha", vec![wanted.clone()]),
            FakeSource::resolving("soda", vec![song("soda", "2", "Blue Sky", "Nova", 200_000)]),
        ];
        // Every probe fails; soda wins only because it is never probed.
        let fetcher = FakeFetcher::new(&[], 403);
        let resolved = resolve_with_fallback(sources, fetcher, cookies(), &wanted, true)
            .await
            .expect("soda is trusted without a probe");
        assert!(resolved.switched);
        assert_eq!(resolved.served_by, "soda");
        assert_eq!(resolved.playability, Playability::Unprobeable);
    }

    #[tokio::test]
    async fn candidate_probes_run_concurrently() {
        let wanted = song("alpha", "1", "Blue Sky", "Nova", 200_000);
        // Four candidates, all refusing, each probe sleeping 120ms. Serial
        // probing needs 480ms+; the bounded pool overlaps them within one batch.
        let mut sources = vec![FakeSource::resolving("alpha", vec![wanted.clone()])];
        for id in ["beta", "gamma", "delta", "epsilon"] {
            sources.push(FakeSource::resolving(
                id,
                vec![song(id, "2", "Blue Sky", "Nova", 200_000)],
            ));
        }
        let fetcher = FakeFetcher::slow(&[], 403, Duration::from_millis(120));
        let started = std::time::Instant::now();
        let probe_fetcher = Arc::clone(&fetcher) as Arc<dyn ByteFetcher>;
        let error = resolve_with_fallback(sources, probe_fetcher, cookies(), &wanted, true)
            .await
            .expect_err("nothing plays");
        assert_eq!(error.code, "PLAYBACK_NO_PLAYABLE_SOURCE");
        assert_eq!(fetcher.probes().len(), 5, "primary plus four candidates");
        assert!(
            fetcher.peak_concurrent.load(Ordering::SeqCst) > 1,
            "candidate probes must overlap"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "probing took {elapsed:?}, which suggests serial probes"
        );
    }

    #[tokio::test]
    async fn a_song_without_a_title_has_no_candidates() {
        let wanted = song("alpha", "1", "   ", "Nova", 200_000);
        let sources = vec![FakeSource::resolving(
            "beta",
            vec![song("beta", "2", "Blue Sky", "Nova", 200_000)],
        )];
        let candidates = find_candidates(sources, &wanted, SOURCE_TIMEOUT).await;
        assert!(candidates.is_empty());
    }

    #[tokio::test]
    async fn an_unknown_primary_source_is_an_error() {
        let wanted = song("nowhere", "1", "Blue Sky", "Nova", 200_000);
        let sources = vec![FakeSource::resolving("alpha", vec![])];
        let fetcher = FakeFetcher::new(&[], 206);
        let error = resolve_with_fallback(sources, fetcher, cookies(), &wanted, true)
            .await
            .expect_err("the requested source is not registered");
        assert_eq!(error.code, "SOURCE_NOT_FOUND");
    }
}
