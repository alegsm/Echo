use rusqlite::{params, Connection, OptionalExtension, Row};

use super::models::{
    HistoryCursor, HistoryMutation, HistoryPage, StorageError, StorageResult, StoredTranscript,
    TranscriptAudioMetadata, TranscriptRecord, TranscriptTiming, MAX_HISTORY_PAGE,
};
use super::EchoStorage;

impl EchoStorage {
    pub fn get_transcript(&self, id: &str) -> StorageResult<Option<StoredTranscript>> {
        validate_record_id(id)?;
        let connection = self.lock()?;
        find(&connection, id)
    }

    pub fn list_history(
        &self,
        limit: u32,
        cursor: Option<HistoryCursor>,
    ) -> StorageResult<HistoryPage> {
        if limit == 0 || limit > MAX_HISTORY_PAGE {
            return Err(StorageError::Validation("history.limit"));
        }
        if let Some(cursor) = cursor.as_ref() {
            validate_record_id(&cursor.id)?;
            super::record_validation::validate_rfc3339(&cursor.created_at)?;
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        if let Some(cursor) = cursor.as_ref() {
            validate_sqlite_timestamp(&transaction, &cursor.created_at)?;
        }
        let retention = super::settings::read(&transaction)?
            .settings
            .history_retention_days;
        let purged = super::retention::purge_with_current_time(&transaction, retention)?;
        let revision = history_revision(&transaction)?;
        let total_count = count_history(&transaction)?;
        let mut records = read_page(&transaction, limit + 1, cursor.as_ref())?;
        let has_more = records.len() > limit as usize;
        if has_more {
            records.pop();
        }
        let next_cursor = has_more
            .then(|| records.last())
            .flatten()
            .map(|stored| HistoryCursor {
                created_at: stored.record.created_at.clone(),
                id: stored.record.id.clone(),
            });
        transaction.commit()?;
        Ok(HistoryPage {
            cleanup_status: if purged
                || self
                    .cleanup_pending
                    .load(std::sync::atomic::Ordering::Relaxed)
            {
                self.cleanup_deleted_pages(&connection)
            } else {
                super::CleanupStatus::Complete
            },
            purged,
            revision,
            records,
            next_cursor,
            total_count,
        })
    }

    pub fn append_transcript(
        &self,
        mut record: TranscriptRecord,
    ) -> StorageResult<HistoryMutation> {
        record.validate()?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        validate_sqlite_timestamp(&transaction, &record.created_at)?;
        if let Some(existing) = find(&transaction, &record.id)? {
            if existing.record == record {
                return Ok(HistoryMutation {
                    cleanup_status: None,
                    history_revision: history_revision(&transaction)?,
                    transcript: Some(existing),
                    record_id: None,
                    affected_count: 0,
                });
            }
            return Err(StorageError::TranscriptConflict);
        }
        insert(&transaction, &record)?;
        let history_revision = increment_history_revision(&transaction)?;
        transaction.commit()?;
        Ok(HistoryMutation {
            cleanup_status: None,
            history_revision,
            transcript: Some(StoredTranscript {
                revision: 1,
                record,
            }),
            record_id: None,
            affected_count: 1,
        })
    }

    pub fn delete_transcript(&self, id: &str, expected: u64) -> StorageResult<HistoryMutation> {
        validate_record_id(id)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let existing = find(&transaction, id)?.ok_or(StorageError::NotFound("transcript"))?;
        ensure_record_revision(expected, existing.revision)?;
        transaction.execute("DELETE FROM transcripts WHERE id=?1", [id])?;
        let history_revision = increment_history_revision(&transaction)?;
        transaction.commit()?;
        Ok(HistoryMutation {
            cleanup_status: Some(self.cleanup_deleted_pages(&connection)),
            history_revision,
            transcript: None,
            record_id: Some(id.to_string()),
            affected_count: 1,
        })
    }

    pub fn clear_history(&self, expected: u64) -> StorageResult<HistoryMutation> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        ensure_history_revision(&transaction, expected)?;
        let changed = transaction.execute("DELETE FROM transcripts", [])?;
        let revision = if changed > 0 {
            increment_history_revision(&transaction)?
        } else {
            expected
        };
        transaction.commit()?;
        Ok(HistoryMutation {
            cleanup_status: Some(self.cleanup_deleted_pages(&connection)),
            history_revision: revision,
            transcript: None,
            record_id: None,
            affected_count: changed as u64,
        })
    }

    pub fn update_transcript_delivery(
        &self,
        id: &str,
        expected: u64,
        insertion_mode: &str,
        error_code: Option<&str>,
    ) -> StorageResult<HistoryMutation> {
        validate_record_id(id)?;
        validate_delivery(insertion_mode, error_code)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let existing = find(&transaction, id)?.ok_or(StorageError::NotFound("transcript"))?;
        ensure_record_revision(expected, existing.revision)?;
        let next_revision = existing.revision + 1;
        let error_code = if existing.record.error_code.as_deref() == Some("provider_failed") {
            Some("provider_failed")
        } else {
            error_code
        };
        transaction.execute(
            "UPDATE transcripts SET insertion_mode=?1,error_code=?2,record_revision=?3,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?4",
            params![insertion_mode, error_code, next_revision, id],
        )?;
        let history_revision = increment_history_revision(&transaction)?;
        let transcript = find(&transaction, id)?;
        transaction.commit()?;
        Ok(HistoryMutation {
            cleanup_status: None,
            history_revision,
            transcript,
            record_id: None,
            affected_count: 1,
        })
    }
}

fn validate_delivery(insertion_mode: &str, error_code: Option<&str>) -> StorageResult<()> {
    let valid = matches!((insertion_mode, error_code), ("pasted" | "copied", None))
        || matches!(
            (insertion_mode, error_code),
            ("failed", Some("clipboard_failed" | "insertion_failed"))
        );
    valid
        .then_some(())
        .ok_or(StorageError::Validation("transcript.delivery"))
}

pub(super) fn count_history(connection: &Connection) -> StorageResult<u64> {
    Ok(connection.query_row("SELECT COUNT(*) FROM transcripts", [], |row| row.get(0))?)
}

pub(super) fn history_revision(connection: &Connection) -> StorageResult<u64> {
    Ok(
        connection.query_row("SELECT revision FROM history_meta WHERE id=1", [], |row| {
            row.get(0)
        })?,
    )
}

pub(super) fn increment_history_revision(connection: &Connection) -> StorageResult<u64> {
    connection.execute("UPDATE history_meta SET revision=revision+1 WHERE id=1", [])?;
    history_revision(connection)
}

pub(super) fn find(connection: &Connection, id: &str) -> StorageResult<Option<StoredTranscript>> {
    Ok(connection
        .query_row(
            &format!("{} WHERE id=?1", select_columns()),
            [id],
            map_transcript,
        )
        .optional()?)
}

pub(super) fn insert(connection: &Connection, record: &TranscriptRecord) -> StorageResult<()> {
    let timings = serde_json::to_string(&record.timings).map_err(|_| StorageError::Database)?;
    connection.execute(
        "INSERT INTO transcripts(id,record_revision,created_at,text,duration_ms,provider_id,provider_model,language,prompt,timings_json,audio_mime_type,audio_byte_size,insertion_mode,source,error_code,updated_at)
         VALUES(?1,1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        params![record.id, record.created_at, record.text, record.duration_ms, record.provider_id, record.provider_model, record.language, record.prompt, timings, record.audio.mime_type, record.audio.byte_size, record.insertion_mode, record.source, record.error_code],
    )?;
    Ok(())
}

fn read_page(
    connection: &Connection,
    limit: u32,
    cursor: Option<&HistoryCursor>,
) -> StorageResult<Vec<StoredTranscript>> {
    let order = " ORDER BY julianday(created_at) DESC,id DESC LIMIT ?1";
    let sql = match cursor {
        Some(_) => format!(
            "{} WHERE julianday(created_at)<julianday(?2) OR \
             (julianday(created_at)=julianday(?2) AND id<?3){order}",
            select_columns()
        ),
        None => format!("{}{order}", select_columns()),
    };
    let mut statement = connection.prepare(&sql)?;
    let rows = match cursor {
        Some(cursor) => {
            statement.query_map(params![limit, cursor.created_at, cursor.id], map_transcript)?
        }
        None => statement.query_map(params![limit], map_transcript)?,
    };
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn map_transcript(row: &Row<'_>) -> rusqlite::Result<StoredTranscript> {
    let timings_json: String = row.get(10)?;
    let timings: TranscriptTiming = serde_json::from_str(&timings_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(StoredTranscript {
        revision: row.get(1)?,
        record: TranscriptRecord {
            id: row.get(0)?,
            created_at: row.get(2)?,
            text: row.get(3)?,
            duration_ms: row.get(4)?,
            provider_id: row.get(5)?,
            provider_model: row.get(6)?,
            language: row.get(7)?,
            prompt: row.get(8)?,
            timings,
            audio: TranscriptAudioMetadata {
                mime_type: row.get(11)?,
                byte_size: row.get(12)?,
            },
            insertion_mode: row.get(13)?,
            source: row.get(14)?,
            error_code: row.get(15)?,
        },
    })
}

fn select_columns() -> &'static str {
    "SELECT id,record_revision,created_at,text,duration_ms,provider_id,provider_model,language,prompt,updated_at,timings_json,audio_mime_type,audio_byte_size,insertion_mode,source,error_code FROM transcripts"
}

fn ensure_record_revision(expected: u64, actual: u64) -> StorageResult<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(StorageError::RevisionConflict {
            resource: "transcript",
            expected,
            actual,
        })
    }
}

fn ensure_history_revision(connection: &Connection, expected: u64) -> StorageResult<()> {
    let actual = history_revision(connection)?;
    if expected == actual {
        Ok(())
    } else {
        Err(StorageError::RevisionConflict {
            resource: "history",
            expected,
            actual,
        })
    }
}

pub(super) fn validate_sqlite_timestamp(connection: &Connection, value: &str) -> StorageResult<()> {
    let parsed: Option<f64> =
        connection.query_row("SELECT julianday(?1)", [value], |row| row.get(0))?;
    parsed
        .is_some()
        .then_some(())
        .ok_or(StorageError::Validation("transcript.createdAt"))
}

fn validate_record_id(id: &str) -> StorageResult<()> {
    if !id.is_empty() && id.len() <= 128 {
        Ok(())
    } else {
        Err(StorageError::Validation("transcript.id"))
    }
}
