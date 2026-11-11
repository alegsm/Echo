use rusqlite::{params, Connection};

use super::models::{RetentionPreview, StorageError, StorageResult};
use super::EchoStorage;

impl EchoStorage {
    #[cfg(test)]
    pub(super) fn purge_history(
        &self,
        retention_days: Option<u32>,
        now: &str,
    ) -> StorageResult<u64> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        purge(&transaction, retention_days, now)?;
        let revision = super::history::history_revision(&transaction)?;
        transaction.commit()?;
        self.cleanup_deleted_pages(&connection);
        Ok(revision)
    }

    pub fn preview_retention(
        &self,
        retention_days: Option<u32>,
    ) -> StorageResult<RetentionPreview> {
        validate_retention(retention_days)?;
        let connection = self.lock()?;
        let total_count = super::history::count_history(&connection)?;
        let delete_count = count_expired(&connection, retention_days)?;
        Ok(RetentionPreview {
            history_revision: super::history::history_revision(&connection)?,
            retention_days,
            delete_count,
            retain_count: total_count.saturating_sub(delete_count),
        })
    }
}

pub(super) fn purge_with_current_time(
    connection: &Connection,
    days: Option<u32>,
) -> StorageResult<bool> {
    let now = super::database::current_timestamp(connection)?;
    purge(connection, days, &now)
}

pub(super) fn purge(connection: &Connection, days: Option<u32>, now: &str) -> StorageResult<bool> {
    validate_retention(days)?;
    let Some(days) = days else {
        return Ok(false);
    };
    validate_now(connection, now)?;
    let modifier = format!("-{days} days");
    let changed = connection.execute(
        "DELETE FROM transcripts WHERE julianday(created_at)<julianday(?1,?2)",
        params![now, modifier],
    )?;
    if changed > 0 {
        super::history::increment_history_revision(connection)?;
    }
    Ok(changed > 0)
}

fn count_expired(connection: &Connection, days: Option<u32>) -> StorageResult<u64> {
    let Some(days) = days else {
        return Ok(0);
    };
    let modifier = format!("-{days} days");
    let now = super::database::current_timestamp(connection)?;
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM transcripts WHERE julianday(created_at)<julianday(?1,?2)",
        params![now, modifier],
        |row| row.get(0),
    )?)
}

fn validate_retention(days: Option<u32>) -> StorageResult<()> {
    if days.is_some_and(|value| !(1..=3650).contains(&value)) {
        Err(StorageError::Validation("historyRetentionDays"))
    } else {
        Ok(())
    }
}

fn validate_now(connection: &Connection, now: &str) -> StorageResult<()> {
    let parsed: Option<f64> =
        connection.query_row("SELECT julianday(?1)", [now], |row| row.get(0))?;
    parsed
        .is_some()
        .then_some(())
        .ok_or(StorageError::Validation("history.now"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_support::{cleanup, initialized, test_database, transcript};
    use std::sync::atomic::Ordering;

    // Keep: test-only retention purges must track and clear pending WAL cleanup
    // just like production purges, using a real reader that blocks truncation.
    #[test]
    fn retention_test_purge_tracks_pending_cleanup_and_retry() {
        let path = test_database("retention-cleanup");
        let storage = initialized(&path);
        storage
            .lock()
            .unwrap()
            .busy_timeout(std::time::Duration::from_millis(1))
            .unwrap();
        storage
            .append_transcript(transcript("expired", "2000-01-01T00:00:00Z"))
            .unwrap();
        let reader = Connection::open(&path).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM transcripts;")
            .unwrap();
        assert_eq!(
            storage
                .purge_history(Some(7), "2026-11-08T00:00:00Z")
                .unwrap(),
            2
        );
        assert!(storage.cleanup_pending.load(Ordering::Relaxed));
        reader.execute_batch("ROLLBACK").unwrap();
        drop(reader);
        assert_eq!(
            storage
                .purge_history(Some(7), "2026-11-08T00:00:00Z")
                .unwrap(),
            2
        );
        assert!(!storage.cleanup_pending.load(Ordering::Relaxed));
        drop(storage);
        cleanup(&path);
    }
}
