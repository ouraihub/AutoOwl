use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::domain::source::MusicSource;
use crate::infrastructure::credentials::{CookieStore, SourceCookie};
use crate::infrastructure::http::ReqwestHttpClient;

/// Every source id the application can build, in display order.
///
/// `local` is excluded: it is served by the local music subsystem rather than
/// a network adapter.
pub const SOURCE_IDS: [&str; 12] = [
    "netease", "qq", "kugou", "kuwo", "migu", "fivesing", "jamendo", "joox", "qianqian", "soda",
    "bilibili", "apple",
];

/// Live adapter instances keyed by source id.
///
/// Adapters read credentials from the shared [`CookieStore`] on every request,
/// so a login only writes the store; instances are never rebuilt.
#[derive(Default)]
pub struct SourceRegistry {
    sources: RwLock<HashMap<String, Arc<dyn MusicSource>>>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, source: Arc<dyn MusicSource>) {
        if let Ok(mut sources) = self.sources.write() {
            sources.insert(source.id().to_owned(), source);
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn MusicSource>> {
        self.sources
            .read()
            .ok()
            .and_then(|sources| sources.get(id).cloned())
    }

    pub fn supports(&self, id: &str) -> bool {
        SOURCE_IDS.contains(&id)
    }

    /// Registered ids in [`SOURCE_IDS`] order.
    pub fn ids(&self) -> Vec<String> {
        let registered = self.sources.read().ok();
        SOURCE_IDS
            .iter()
            .filter(|id| {
                registered
                    .as_ref()
                    .map(|sources| sources.contains_key(**id))
                    .unwrap_or(false)
            })
            .map(|id| (*id).to_owned())
            .collect()
    }

    /// Builds every adapter against a shared cookie store and HTTP client.
    pub fn with_sources(cookies: Arc<CookieStore>) -> Self {
        let registry = Self::new();
        let client = ReqwestHttpClient::default();
        for id in SOURCE_IDS {
            let cookie = SourceCookie::new(Arc::clone(&cookies), id);
            if let Some(source) = build_source(id, client.clone(), cookie) {
                registry.register(source);
            }
        }
        registry
    }
}

fn build_source(
    id: &str,
    client: ReqwestHttpClient,
    cookie: SourceCookie,
) -> Option<Arc<dyn MusicSource>> {
    Some(match id {
        "netease" => Arc::new(super::netease::NeteaseSource::new(client, cookie)),
        "qq" => Arc::new(super::qq::QqSource::new(client, cookie)),
        "kugou" => Arc::new(super::kugou::KugouSource::new(client, cookie)),
        "kuwo" => Arc::new(super::kuwo::KuwoSource::new(client, cookie)),
        "migu" => Arc::new(super::migu::MiguSource::new(client, cookie)),
        "soda" => Arc::new(super::soda::SodaSource::new(client, cookie)),
        "bilibili" => Arc::new(super::bilibili::BilibiliSource::new(client, cookie)),
        "joox" => Arc::new(super::joox::JooxSource::new(client, cookie)),
        "qianqian" => Arc::new(super::qianqian::QianqianSource::new(client, cookie)),
        "jamendo" => Arc::new(super::jamendo::JamendoSource::new(client, cookie)),
        "apple" => Arc::new(super::apple::AppleSource::new(client, cookie)),
        "fivesing" => Arc::new(super::fivesing::FivesingSource::new(client, cookie)),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{SourceRegistry, SOURCE_IDS};
    use crate::infrastructure::credentials::CookieStore;
    use std::sync::Arc;

    #[test]
    fn every_declared_source_is_registered_in_order() {
        let registry = SourceRegistry::with_sources(Arc::new(CookieStore::default()));
        assert_eq!(registry.ids(), SOURCE_IDS.to_vec());
    }

    #[test]
    fn adapter_ids_match_their_registry_key() {
        let registry = SourceRegistry::with_sources(Arc::new(CookieStore::default()));
        for id in SOURCE_IDS {
            let source = registry.get(id).unwrap_or_else(|| unreachable!("{id}"));
            assert_eq!(source.id(), id);
        }
    }

    #[test]
    fn cookie_updates_reach_live_adapters_without_rebuilding() {
        let cookies = Arc::new(CookieStore::default());
        let registry = SourceRegistry::with_sources(Arc::clone(&cookies));
        let before = registry.get("netease").unwrap_or_else(|| unreachable!());
        assert!(cookies.save("netease", "MUSIC_U=secret").is_ok());
        let after = registry.get("netease").unwrap_or_else(|| unreachable!());
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(cookies.get("netease"), "MUSIC_U=secret");
    }

    #[test]
    fn unknown_source_is_unsupported() {
        let registry = SourceRegistry::with_sources(Arc::new(CookieStore::default()));
        assert!(!registry.supports("unknown"));
        assert!(registry.get("unknown").is_none());
        assert!(registry.supports("apple"));
    }
}
