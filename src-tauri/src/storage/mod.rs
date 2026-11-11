//! Owner-only SQLite persistence for Echo settings and transcript history.
//!
//! The facade serializes revision-checked transactions across windows. Native
//! setup publishes storage only after initialization and settings reads succeed.
//! Deleted transcripts are overwritten from this version on. No compaction of
//! pre-upgrade free pages is performed. A pending cleanup means deletion committed
//! but another reader or an I/O error prevented WAL truncation; the next purge or
//! launch retries it. This does not erase backups.

mod database;
mod history;
mod models;
mod record_validation;
mod retention;
mod settings;
mod validation;

use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, MutexGuard,
};

use rusqlite::Connection;

pub use models::*;

pub struct EchoStorage {
    pub(super) connection: Mutex<Connection>,
    cleanup_pending: AtomicBool,
}

impl EchoStorage {
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let connection = database::open(path.as_ref())?;
        Ok(Self {
            connection: Mutex::new(connection),
            cleanup_pending: AtomicBool::new(false),
        })
    }

    pub fn initialize(&self) -> StorageResult<CleanupStatus> {
        let mut connection = self.lock()?;
        database::initialize(&connection)?;
        let transaction = connection.transaction()?;
        settings::ensure_default(&transaction)?;
        let retention = settings::read(&transaction)?
            .settings
            .history_retention_days;
        retention::purge_with_current_time(&transaction, retention)?;
        transaction.commit()?;
        Ok(self.cleanup_deleted_pages(&connection))
    }

    // Call only with the connection mutex held, so cleanup and pending state
    // advance together across deletion, retention, settings and startup.
    pub(super) fn cleanup_deleted_pages(&self, connection: &Connection) -> CleanupStatus {
        let status = database::cleanup_deleted_pages(connection);
        self.cleanup_pending
            .store(status == CleanupStatus::Pending, Ordering::Relaxed);
        status
    }

    pub fn current_timestamp(&self) -> StorageResult<String> {
        let connection = self.lock()?;
        database::current_timestamp(&connection)
    }

    pub(super) fn lock(&self) -> StorageResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| StorageError::LockUnavailable)
    }
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "timestamp_tests.rs"]
mod timestamp_tests;
