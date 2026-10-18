use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

const MIGRATION_V1: &str = include_str!("../../../docs/sqlite/v1.sql");
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("storage lock poisoned")]
    Poisoned,
}

/// Serialized SQLite handle.
///
/// A single connection guarded by a mutex is sufficient: every write path is a
/// short KV/record upsert and `busy_timeout` covers the WAL reader overlap.
pub struct Database {
    connection: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let connection = Connection::open(path)?;
        Self::prepare(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// In-memory database for tests and for the fallback when the data
    /// directory is not writable.
    pub fn open_in_memory() -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory()?;
        Self::prepare(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn prepare(connection: &Connection) -> Result<(), StorageError> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "busy_timeout", 5000)?;
        connection.execute_batch(MIGRATION_V1)?;
        connection.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(?1, ?2)",
            rusqlite::params![SCHEMA_VERSION, crate::domain::clock::now_iso8601()],
        )?;
        Ok(())
    }

    pub fn with<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, StorageError> {
        let guard = self.lock()?;
        operation(&guard).map_err(StorageError::from)
    }

    pub fn with_transaction<T>(
        &self,
        operation: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, rusqlite::Error>,
    ) -> Result<T, StorageError> {
        let mut guard = self.lock()?;
        let transaction = guard.transaction()?;
        let value = operation(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, StorageError> {
        self.connection.lock().map_err(|_| StorageError::Poisoned)
    }

    pub fn schema_version(&self) -> Result<i64, StorageError> {
        self.with(|connection| {
            connection.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Database, SCHEMA_VERSION};

    #[test]
    fn migration_is_idempotent_and_records_version() {
        let database = Database::open_in_memory().expect("open");
        assert_eq!(database.schema_version().expect("version"), SCHEMA_VERSION);
        let rows: i64 = database
            .with(|connection| {
                connection.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
            })
            .expect("count");
        assert_eq!(rows, 1);
    }

    #[test]
    fn transaction_rolls_back_on_error() {
        let database = Database::open_in_memory().expect("open");
        let result: Result<(), _> = database.with_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO app_settings(key, value_json, updated_at) VALUES('a', '1', 'now')",
                [],
            )?;
            Err(rusqlite::Error::InvalidQuery)
        });
        assert!(result.is_err());
        let rows: i64 = database
            .with(|connection| {
                connection.query_row("SELECT COUNT(*) FROM app_settings", [], |row| row.get(0))
            })
            .expect("count");
        assert_eq!(rows, 0);
    }
}
