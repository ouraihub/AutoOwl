use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::domain::clock::now_iso8601;
use crate::infrastructure::secret::{self, Opened};
use crate::infrastructure::storage::Database;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub source: String,
    pub configured: bool,
    pub qr_supported: bool,
}

/// Read-through cookie cache shared by every music source adapter.
///
/// Cookies are the only credential the sources carry, and adapters read them
/// on every request, so the hot path is an in-memory `RwLock` lookup. Writes go
/// to SQLite first so a restart keeps the login, then update the cache.
///
/// Values are secrets: `status` never exposes them and `Debug` is not derived.
#[derive(Default)]
pub struct CookieStore {
    values: RwLock<HashMap<String, String>>,
    database: Option<Arc<Database>>,
}

impl CookieStore {
    /// Builds a store backed by SQLite, loading any persisted cookies.
    pub fn with_database(database: Arc<Database>) -> Self {
        let values = load(&database).unwrap_or_default();
        Self {
            values: RwLock::new(values),
            database: Some(database),
        }
    }

    /// Current cookie for a source; empty when not configured.
    pub fn get(&self, source: &str) -> String {
        self.values
            .read()
            .ok()
            .and_then(|values| values.get(source).cloned())
            .unwrap_or_default()
    }

    pub fn save(&self, source: &str, value: &str) -> Result<(), String> {
        if source.trim().is_empty() || value.trim().is_empty() {
            return Err("凭据不能为空".to_owned());
        }
        if let Some(database) = &self.database {
            let sealed = secret::seal(value)?;
            database
                .with(|connection| {
                    connection.execute(
                        "INSERT INTO credentials(source, encrypted_value, updated_at) \
                         VALUES(?1, ?2, ?3) \
                         ON CONFLICT(source) DO UPDATE SET \
                         encrypted_value = excluded.encrypted_value, \
                         updated_at = excluded.updated_at",
                        rusqlite::params![source, sealed, now_iso8601()],
                    )
                })
                .map_err(|error| format!("凭据保存失败: {error}"))?;
        }
        self.values
            .write()
            .map_err(|_| "凭据存储锁已损坏".to_owned())?
            .insert(source.to_owned(), value.to_owned());
        Ok(())
    }

    pub fn clear(&self, source: &str) -> Result<(), String> {
        if let Some(database) = &self.database {
            database
                .with(|connection| {
                    connection.execute(
                        "DELETE FROM credentials WHERE source = ?1",
                        rusqlite::params![source],
                    )
                })
                .map_err(|error| format!("凭据清除失败: {error}"))?;
        }
        self.values
            .write()
            .map_err(|_| "凭据存储锁已损坏".to_owned())?
            .remove(source);
        Ok(())
    }

    pub fn status(&self, source: &str, qr_supported: bool) -> AuthStatus {
        AuthStatus {
            source: source.to_owned(),
            configured: !self.get(source).is_empty(),
            qr_supported,
        }
    }

    /// Source ids that currently hold a cookie.
    pub fn configured_sources(&self) -> Vec<String> {
        let mut sources: Vec<String> = self
            .values
            .read()
            .map(|values| values.keys().cloned().collect())
            .unwrap_or_default();
        sources.sort();
        sources
    }
}

/// Loads every stored cookie, re-sealing rows written before encryption existed.
/// A row that cannot be decrypted (other Windows user, tampering) is skipped, so
/// that source shows as "未配置" and the user simply logs in again.
fn load(database: &Database) -> Result<HashMap<String, String>, String> {
    let rows: Vec<(String, Vec<u8>)> = database
        .with(|connection| {
            let mut statement =
                connection.prepare("SELECT source, encrypted_value FROM credentials")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })
        .map_err(|error| error.to_string())?;

    let mut values = HashMap::with_capacity(rows.len());
    for (source, stored) in rows {
        match secret::open(&stored) {
            Ok(Opened::Sealed(value)) => {
                values.insert(source, value);
            }
            Ok(Opened::Legacy(value)) => {
                reseal(database, &source, &value);
                values.insert(source, value);
            }
            Err(error) => tracing::warn!(event = "credential_unreadable", %source, %error),
        }
    }
    Ok(values)
}

fn reseal(database: &Database, source: &str, value: &str) {
    let result = secret::seal(value).and_then(|sealed| {
        database
            .with(|connection| {
                connection.execute(
                    "UPDATE credentials SET encrypted_value = ?1 WHERE source = ?2",
                    rusqlite::params![sealed, source],
                )
            })
            .map_err(|error| error.to_string())
    });
    if let Err(error) = result {
        tracing::warn!(event = "credential_reseal_failed", %source, %error);
    }
}

/// A cookie handle already bound to one source id.
///
/// Adapters hold this instead of a `String` so a login is visible to the live
/// adapter instance without rebuilding it.
#[derive(Clone)]
pub struct SourceCookie {
    store: Arc<CookieStore>,
    source: &'static str,
}

impl SourceCookie {
    pub fn new(store: Arc<CookieStore>, source: &'static str) -> Self {
        Self { store, source }
    }

    /// Current cookie value; empty when the source is not logged in.
    pub fn value(&self) -> String {
        self.store.get(self.source)
    }

    pub fn is_empty(&self) -> bool {
        self.value().is_empty()
    }

    /// Handle with no credential, for tests and unauthenticated sources.
    pub fn empty() -> Self {
        Self::new(Arc::new(CookieStore::default()), "test")
    }

    /// Handle holding a fixed value, for tests.
    #[cfg(test)]
    pub fn fixed(value: &str) -> Self {
        let store = CookieStore::default();
        let _ = store.save("test", value);
        Self::new(Arc::new(store), "test")
    }
}

#[cfg(test)]
mod tests {
    use super::CookieStore;
    use crate::infrastructure::storage::Database;
    use std::sync::Arc;

    #[test]
    fn status_never_contains_secret() {
        let store = CookieStore::default();
        assert!(store.save("netease", "secret-cookie").is_ok());
        let status = store.status("netease", true);
        assert!(status.configured);
        let encoded = serde_json::to_string(&status).unwrap_or_default();
        assert!(!encoded.contains("secret-cookie"));
    }

    #[test]
    fn blank_values_are_rejected() {
        let store = CookieStore::default();
        assert!(store.save("netease", "   ").is_err());
        assert!(store.save("  ", "cookie").is_err());
        assert!(store.get("netease").is_empty());
    }

    #[test]
    fn cookies_survive_a_reload_from_sqlite() {
        let database = Arc::new(Database::open_in_memory().expect("open"));
        let store = CookieStore::with_database(Arc::clone(&database));
        assert!(store.save("qq", "uin=123").is_ok());

        let reloaded = CookieStore::with_database(Arc::clone(&database));
        assert_eq!(reloaded.get("qq"), "uin=123");
        assert_eq!(reloaded.configured_sources(), vec!["qq".to_owned()]);

        assert!(reloaded.clear("qq").is_ok());
        let after_clear = CookieStore::with_database(database);
        assert!(after_clear.get("qq").is_empty());
    }

    #[test]
    fn legacy_plaintext_rows_are_resealed_on_load() {
        let database = Arc::new(Database::open_in_memory().expect("open"));
        database
            .with(|c| {
                c.execute(
                    "INSERT INTO credentials(source, encrypted_value, updated_at) VALUES('qq', ?1, '')",
                    rusqlite::params![b"uin=123".to_vec()],
                )
            })
            .expect("insert legacy row");

        let store = CookieStore::with_database(Arc::clone(&database));
        assert_eq!(store.get("qq"), "uin=123");

        let stored: Vec<u8> = database
            .with(|c| c.query_row("SELECT encrypted_value FROM credentials", [], |r| r.get(0)))
            .expect("read back");
        assert_ne!(stored, b"uin=123", "legacy row must be re-sealed");
        assert_eq!(CookieStore::with_database(database).get("qq"), "uin=123");
    }

    #[test]
    fn saving_twice_updates_instead_of_duplicating() {
        let database = Arc::new(Database::open_in_memory().expect("open"));
        let store = CookieStore::with_database(Arc::clone(&database));
        assert!(store.save("kugou", "first").is_ok());
        assert!(store.save("kugou", "second").is_ok());
        assert_eq!(store.get("kugou"), "second");
        assert_eq!(CookieStore::with_database(database).get("kugou"), "second");
    }
}
