use std::fs::{self, OpenOptions};
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};

use super::models::{CleanupStatus, StorageError, StorageResult};

pub fn open(path: &Path) -> StorageResult<Connection> {
    let parent = path
        .parent()
        .ok_or(StorageError::Validation("databasePath"))?;
    reject_redirects(parent)?;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut artifact = path.as_os_str().to_os_string();
        artifact.push(suffix);
        reject_redirects(Path::new(&artifact))?;
    }
    create_private_directory(parent)?;
    let connection = Connection::open(path)?;
    enable_secure_delete(&connection)?;
    harden_file(path)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(connection)
}

// Check live leaf paths before SQLite opens or changes their permissions.
// Redirected ancestors remain supported, as with OS-selected app-data roots.
fn reject_redirects(path: &Path) -> StorageResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| part == std::path::Component::ParentDir)
    {
        return Err(StorageError::Database);
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(StorageError::Database);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(StorageError::Database);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(super) fn enable_secure_delete(connection: &Connection) -> StorageResult<()> {
    // Must precede schema changes, WAL setup, and publication of EchoStorage.
    connection.pragma_update(None, "secure_delete", "ON")?;
    let secure_delete: u32 = connection.pragma_query_value(None, "secure_delete", |r| r.get(0))?;
    if secure_delete != 1 {
        return Err(StorageError::Database);
    }
    Ok(())
}

/// Run outside the transaction while holding the storage mutex. Deletes already
/// committed must remain successful even if another reader keeps old WAL frames.
/// Every purge retries this cleanup, including empty purges and startup.
pub(super) fn cleanup_deleted_pages(connection: &Connection) -> CleanupStatus {
    let checkpoint = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok((
            row.get::<_, i32>(0)?,
            row.get::<_, i32>(1)?,
            row.get::<_, i32>(2)?,
        ))
    });
    match checkpoint {
        Ok((0, 0, 0)) => CleanupStatus::Complete,
        _ => CleanupStatus::Pending,
    }
}

pub fn initialize(connection: &Connection) -> StorageResult<()> {
    connection.execute(
        "CREATE TABLE IF NOT EXISTS schema_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)",
        [],
    )?;
    verify_schema_version(connection)?;
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS settings(
           id INTEGER PRIMARY KEY CHECK(id=1), schema_version INTEGER NOT NULL,
           revision INTEGER NOT NULL, value_json TEXT NOT NULL, updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS history_meta(
           id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS transcripts(
           id TEXT PRIMARY KEY, record_revision INTEGER NOT NULL, created_at TEXT NOT NULL,
           text TEXT NOT NULL, duration_ms INTEGER NOT NULL CHECK(duration_ms>=0),
           provider_id TEXT NOT NULL, provider_model TEXT, language TEXT, prompt TEXT,
           timings_json TEXT NOT NULL, audio_mime_type TEXT NOT NULL,
           audio_byte_size INTEGER NOT NULL CHECK(audio_byte_size>=0),
           insertion_mode TEXT NOT NULL, source TEXT NOT NULL, error_code TEXT,
           updated_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS transcripts_created_at
           ON transcripts(created_at DESC, id DESC);
         CREATE TABLE IF NOT EXISTS legacy_migrations(
           migration_id TEXT PRIMARY KEY, status TEXT NOT NULL,
           source_checksum TEXT NOT NULL, receipt_json TEXT NOT NULL, updated_at TEXT NOT NULL
         );
         INSERT OR IGNORE INTO schema_meta(key,value) VALUES('schema_version','1');
         INSERT OR IGNORE INTO history_meta(id,revision) VALUES(1,0);
         COMMIT;",
    )?;
    harden_sidecar_files(connection)
}

fn verify_schema_version(connection: &Connection) -> StorageResult<()> {
    let version: Option<String> = connection
        .query_row(
            "SELECT value FROM schema_meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match version.as_deref() {
        None | Some("1") => Ok(()),
        Some(_) => Err(StorageError::Validation("schemaVersion")),
    }
}

pub fn current_timestamp(connection: &Connection) -> StorageResult<String> {
    Ok(
        connection.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |row| {
            row.get(0)
        })?,
    )
}

fn create_private_directory(path: &Path) -> StorageResult<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn harden_sidecar_files(connection: &Connection) -> StorageResult<()> {
    let path = connection.path().ok_or(StorageError::Database)?;
    for suffix in ["", "-wal", "-shm"] {
        let candidate = std::path::PathBuf::from(format!("{path}{suffix}"));
        if candidate.exists() {
            harden_file(&candidate)?;
        }
    }
    Ok(())
}

fn harden_file(path: &Path) -> StorageResult<()> {
    if !path.exists() {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        drop(options.open(path)?);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
