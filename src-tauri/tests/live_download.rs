//! End-to-end download proof: search a live source, resolve it, pull the bytes
//! through the real transfer path and assert the payload is decodable audio.
//!
//! Ignored by default because it hits the public music services.

use autoowl_lib::application::download::{fetch_audio, CancelFlag, TransferProgress};
use autoowl_lib::domain::source::{MusicSource, SearchQuery};
use autoowl_lib::infrastructure::credentials::SourceCookie;
use autoowl_lib::infrastructure::http::{source_headers, ReqwestHttpClient};
use autoowl_lib::infrastructure::sources::{
    bilibili::BilibiliSource, kugou::KugouSource, netease::NeteaseSource, qq::QqSource,
};

/// Title, byte count and detected extension of a completed download.
type Downloaded = (String, usize, &'static str);

/// Per-source download outcome: the payload summary or the reason it failed.
type Outcome = Result<Downloaded, String>;

/// Downloads the first resolvable hit and returns the byte count.
async fn download_first(source: &dyn MusicSource, query: &str) -> Outcome {
    let search = source
        .search_songs(SearchQuery {
            query: query.into(),
            page: 1,
            page_size: 30,
            sources: vec![source.id().to_owned()],
        })
        .await
        .map_err(|error| format!("search failed: {error:?}"))?;
    if search.items.is_empty() {
        return Err("search returned no rows".into());
    }
    let client = ReqwestHttpClient::default();
    let headers = source_headers(source.id(), "");
    let mut last = String::from("no hit resolved");
    for item in search.items {
        let title = item.title.clone();
        let resource = match source.download_url(item.reference).await {
            Ok(resource) => resource,
            Err(error) => {
                last = format!("{}: {}", title, error.message);
                continue;
            }
        };
        let cancel = CancelFlag::default();
        let progress = TransferProgress::default();
        match fetch_audio(&client, &resource.url, &headers, &cancel, &progress).await {
            Ok(payload) => return Ok((title, payload.data.len(), payload.extension)),
            Err(error) => last = format!("{title}: transfer failed: {error:?}"),
        }
    }
    Err(last)
}

#[tokio::test]
#[ignore = "downloads real audio from public services"]
async fn downloads_audio_from_every_working_source() {
    let netease = NeteaseSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
    let kugou = KugouSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
    let qq = QqSource::new(ReqwestHttpClient::default(), SourceCookie::empty());
    let bilibili = BilibiliSource::new(ReqwestHttpClient::default(), SourceCookie::empty());

    let attempts: Vec<(&str, Outcome)> = vec![
        ("netease", download_first(&netease, "千千阙歌").await),
        ("kugou", download_first(&kugou, "千千阙歌").await),
        ("qq", download_first(&qq, "千千阙歌").await),
        ("bilibili", download_first(&bilibili, "千千阙歌").await),
    ];

    let mut succeeded = Vec::new();
    for (id, outcome) in &attempts {
        match outcome {
            Ok((title, bytes, ext)) => {
                println!("{id}: OK {title} -> {bytes} bytes .{ext}");
                assert!(
                    *bytes > 256 * 1024,
                    "{id} returned {bytes} bytes, too small to be a full track"
                );
                succeeded.push(*id);
            }
            Err(reason) => println!("{id}: FAILED {reason}"),
        }
    }
    assert!(
        !succeeded.is_empty(),
        "no source produced a downloadable track: {attempts:?}"
    );
}
