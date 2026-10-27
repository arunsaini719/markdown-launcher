use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;

use super::{document::Document, scanner::DiscoveredFile};

const LATEST_SCHEMA_VERSION: i64 = 2;
const MAX_SEARCH_RESULTS: usize = 100;
const MAX_QUERY_TERMS: usize = 16;
const MAX_QUERY_TERM_CHARS: usize = 64;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const MIGRATION_1: &str = r#"
    CREATE TABLE documents (
        id                  INTEGER PRIMARY KEY,
        canonical_path      TEXT NOT NULL UNIQUE,
        metadata_id         TEXT,
        title               TEXT NOT NULL,
        tags_json           TEXT NOT NULL DEFAULT '[]',
        body                TEXT NOT NULL,
        modified_at_ns      INTEGER NOT NULL,
        file_size           INTEGER NOT NULL CHECK (file_size >= 0),
        content_hash        TEXT NOT NULL,
        indexed_at_unix_ms  INTEGER NOT NULL
    );

    CREATE INDEX documents_modified_at_idx
        ON documents (modified_at_ns, file_size);

    CREATE VIRTUAL TABLE documents_fts USING fts5(
        title,
        tags,
        body,
        content = 'documents',
        content_rowid = 'id',
        tokenize = 'unicode61 remove_diacritics 2'
    );

    CREATE TRIGGER documents_after_insert AFTER INSERT ON documents BEGIN
        INSERT INTO documents_fts(rowid, title, tags, body)
        VALUES (new.id, new.title, new.tags_json, new.body);
    END;

    CREATE TRIGGER documents_after_delete AFTER DELETE ON documents BEGIN
        INSERT INTO documents_fts(documents_fts, rowid, title, tags, body)
        VALUES ('delete', old.id, old.title, old.tags_json, old.body);
    END;

    CREATE TRIGGER documents_after_update AFTER UPDATE ON documents BEGIN
        INSERT INTO documents_fts(documents_fts, rowid, title, tags, body)
        VALUES ('delete', old.id, old.title, old.tags_json, old.body);
        INSERT INTO documents_fts(rowid, title, tags, body)
        VALUES (new.id, new.title, new.tags_json, new.body);
    END;
"#;

const MIGRATION_2: &str = r#"
    DROP TRIGGER documents_after_insert;
    DROP TRIGGER documents_after_delete;
    DROP TRIGGER documents_after_update;
    DROP TABLE documents_fts;

    CREATE VIRTUAL TABLE documents_fts USING fts5(
        title,
        tags_json,
        body,
        content = 'documents',
        content_rowid = 'id',
        tokenize = 'unicode61 remove_diacritics 2'
    );

    CREATE TRIGGER documents_after_insert AFTER INSERT ON documents BEGIN
        INSERT INTO documents_fts(rowid, title, tags_json, body)
        VALUES (new.id, new.title, new.tags_json, new.body);
    END;

    CREATE TRIGGER documents_after_delete AFTER DELETE ON documents BEGIN
        INSERT INTO documents_fts(documents_fts, rowid, title, tags_json, body)
        VALUES ('delete', old.id, old.title, old.tags_json, old.body);
    END;

    CREATE TRIGGER documents_after_update AFTER UPDATE ON documents BEGIN
        INSERT INTO documents_fts(documents_fts, rowid, title, tags_json, body)
        VALUES ('delete', old.id, old.title, old.tags_json, old.body);
        INSERT INTO documents_fts(rowid, title, tags_json, body)
        VALUES (new.id, new.title, new.tags_json, new.body);
    END;

    INSERT INTO documents_fts(rowid, title, tags_json, body)
    SELECT id, title, tags_json, body FROM documents;
"#;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub canonical_path: std::path::PathBuf,
    pub title: String,
    pub tags: Vec<String>,
    pub excerpt: String,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseRecovery {
    pub quarantined_path: PathBuf,
}

#[derive(Debug)]
pub struct IndexDatabase {
    connection: Connection,
}

impl IndexDatabase {
    pub fn open(path: &Path) -> Result<Self, DatabaseError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| DatabaseError::CreateDirectory {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let connection = Connection::open(path).map_err(|source| DatabaseError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        Self::prepare(connection)
    }

    pub fn open_with_recovery(
        path: &Path,
    ) -> Result<(Self, Option<DatabaseRecovery>), DatabaseError> {
        match Self::open(path).and_then(|database| {
            database.validate_integrity()?;
            Ok(database)
        }) {
            Ok(database) => Ok((database, None)),
            Err(error) if path.is_file() && error.is_recoverable_corruption() => {
                let quarantined_path = quarantine_database(path)?;
                let database = Self::open(path)?;
                Ok((database, Some(DatabaseRecovery { quarantined_path })))
            }
            Err(error) => Err(error),
        }
    }

    pub fn open_in_memory() -> Result<Self, DatabaseError> {
        Self::prepare(Connection::open_in_memory()?)
    }

    pub fn schema_version(&self) -> Result<i64, DatabaseError> {
        Ok(self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    pub fn document_count(&self) -> Result<usize, DatabaseError> {
        let count: i64 =
            self.connection
                .query_row("SELECT count(*) FROM documents", [], |row| row.get(0))?;
        usize::try_from(count).map_err(|_| DatabaseError::NumericOverflow("document count"))
    }

    pub fn clear_documents(&mut self) -> Result<usize, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let removed = transaction.execute("DELETE FROM documents", [])?;
        transaction.commit()?;
        Ok(removed)
    }

    pub fn document_paths(&self) -> Result<Vec<PathBuf>, DatabaseError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT canonical_path FROM documents")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0).map(PathBuf::from))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn is_current(&self, file: &DiscoveredFile) -> Result<bool, DatabaseError> {
        let path = path_as_str(&file.canonical_path)?;
        let stored = self
            .connection
            .query_row(
                "SELECT modified_at_ns, file_size FROM documents WHERE canonical_path = ?1",
                [path],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let file_size = i64::try_from(file.file_size)
            .map_err(|_| DatabaseError::NumericOverflow("file size"))?;

        Ok(stored == Some((file.modified_at_ns, file_size)))
    }

    pub fn remove_missing_documents(
        &mut self,
        completed_roots: &[PathBuf],
        observed_paths: &HashSet<PathBuf>,
    ) -> Result<usize, DatabaseError> {
        let stale_paths = self
            .document_paths()?
            .into_iter()
            .filter(|path| !observed_paths.contains(path))
            .filter(|path| {
                completed_roots.iter().any(|root| path.starts_with(root))
                    || matches!(
                        std::fs::symlink_metadata(path),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound
                    )
            })
            .collect::<Vec<_>>();
        self.remove_paths(&stale_paths)
    }

    pub fn remove_documents_outside_roots(
        &mut self,
        canonical_roots: &[PathBuf],
    ) -> Result<usize, DatabaseError> {
        let paths = self
            .document_paths()?
            .into_iter()
            .filter(|path| !canonical_roots.iter().any(|root| path.starts_with(root)))
            .collect::<Vec<_>>();
        self.remove_paths(&paths)
    }

    pub fn remove_document(&mut self, path: &Path) -> Result<usize, DatabaseError> {
        self.remove_paths(&[path.to_path_buf()])
    }

    pub fn remove_paths_under(&mut self, changed_path: &Path) -> Result<usize, DatabaseError> {
        let paths = self
            .document_paths()?
            .into_iter()
            .filter(|path| path == changed_path || path.starts_with(changed_path))
            .collect::<Vec<_>>();
        self.remove_paths(&paths)
    }

    pub fn upsert_documents(&mut self, documents: &[Document]) -> Result<usize, DatabaseError> {
        if documents.is_empty() {
            return Ok(0);
        }

        let indexed_at = unix_time_millis()?;
        let mut updated = 0;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO documents (
                    canonical_path, metadata_id, title, tags_json, body,
                    modified_at_ns, file_size, content_hash, indexed_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(canonical_path) DO UPDATE SET
                    metadata_id = excluded.metadata_id,
                    title = excluded.title,
                    tags_json = excluded.tags_json,
                    body = excluded.body,
                    modified_at_ns = excluded.modified_at_ns,
                    file_size = excluded.file_size,
                    content_hash = excluded.content_hash,
                    indexed_at_unix_ms = excluded.indexed_at_unix_ms
                 WHERE documents.content_hash IS NOT excluded.content_hash
                    OR documents.modified_at_ns IS NOT excluded.modified_at_ns
                    OR documents.file_size IS NOT excluded.file_size
                    OR documents.metadata_id IS NOT excluded.metadata_id
                    OR documents.title IS NOT excluded.title
                    OR documents.tags_json IS NOT excluded.tags_json
                    OR documents.body IS NOT excluded.body",
            )?;

            for document in documents {
                let path = path_as_str(&document.canonical_path)?;
                let tags = serde_json::to_string(&document.tags)?;
                let file_size = i64::try_from(document.file_size)
                    .map_err(|_| DatabaseError::NumericOverflow("file size"))?;
                updated += statement.execute(params![
                    path,
                    document.metadata_id.as_deref(),
                    &document.title,
                    tags,
                    &document.body,
                    document.modified_at_ns,
                    file_size,
                    &document.content_hash,
                    indexed_at,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(updated)
    }

    fn remove_paths(&mut self, paths: &[PathBuf]) -> Result<usize, DatabaseError> {
        if paths.is_empty() {
            return Ok(0);
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = {
            let mut statement =
                transaction.prepare_cached("DELETE FROM documents WHERE canonical_path = ?1")?;
            let mut removed = 0;
            for path in paths {
                removed += statement.execute([path_as_str(path)?])?;
            }
            removed
        };
        transaction.commit()?;
        Ok(removed)
    }

    pub fn search(
        &self,
        query: &str,
        requested_limit: usize,
    ) -> Result<Vec<SearchResult>, DatabaseError> {
        let limit = requested_limit.min(MAX_SEARCH_RESULTS);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit)
            .map_err(|_| DatabaseError::NumericOverflow("search result limit"))?;

        match build_fts_query(query) {
            Some(fts_query) => self.search_fts(&fts_query, limit),
            None if query.trim().is_empty() => self.list_documents(limit),
            None => Ok(Vec::new()),
        }
    }

    fn search_fts(&self, query: &str, limit: i64) -> Result<Vec<SearchResult>, DatabaseError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT
                documents.canonical_path,
                documents.title,
                documents.tags_json,
                snippet(documents_fts, 2, '', '', ' ... ', 24),
                bm25(documents_fts, 10.0, 5.0, 1.0) AS score
             FROM documents_fts
             JOIN documents ON documents.id = documents_fts.rowid
             WHERE documents_fts MATCH ?1
             ORDER BY score, documents.title COLLATE NOCASE, documents.canonical_path
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![query, limit], search_result_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn list_documents(&self, limit: i64) -> Result<Vec<SearchResult>, DatabaseError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT
                canonical_path,
                title,
                tags_json,
                substr(body, 1, 240),
                0.0
             FROM documents
             ORDER BY title COLLATE NOCASE, canonical_path
             LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], search_result_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn prepare(connection: Connection) -> Result<Self, DatabaseError> {
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;

        let mut database = Self { connection };
        database.migrate()?;
        database.validate_schema()?;
        Ok(database)
    }

    fn validate_schema(&self) -> Result<(), DatabaseError> {
        self.connection
            .prepare("SELECT id, canonical_path FROM documents LIMIT 0")
            .map_err(DatabaseError::Validation)?;
        self.connection
            .prepare("SELECT rowid FROM documents_fts LIMIT 0")
            .map_err(DatabaseError::Validation)?;
        Ok(())
    }

    fn validate_integrity(&self) -> Result<(), DatabaseError> {
        let integrity: String = self
            .connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
        if integrity == "ok" {
            Ok(())
        } else {
            Err(DatabaseError::Integrity(integrity))
        }
    }

    fn migrate(&mut self) -> Result<(), DatabaseError> {
        let current_version = self.schema_version()?;
        if current_version > LATEST_SCHEMA_VERSION {
            return Err(DatabaseError::SchemaTooNew {
                found: current_version,
                supported: LATEST_SCHEMA_VERSION,
            });
        }
        if current_version == LATEST_SCHEMA_VERSION {
            return Ok(());
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut migrated_version = current_version;

        if migrated_version < 1 {
            transaction
                .execute_batch(MIGRATION_1)
                .map_err(|source| DatabaseError::Migration { version: 1, source })?;
            migrated_version = 1;
        }
        if migrated_version < 2 {
            transaction
                .execute_batch(MIGRATION_2)
                .map_err(|source| DatabaseError::Migration { version: 2, source })?;
            migrated_version = 2;
        }

        transaction.pragma_update(None, "user_version", migrated_version)?;
        transaction.commit()?;
        Ok(())
    }
}

fn quarantine_database(path: &Path) -> Result<PathBuf, DatabaseError> {
    let timestamp = unix_time_millis()?;
    let mut quarantined_path = PathBuf::from(format!("{}.corrupt-{timestamp}", path.display()));
    let mut collision = 0_u32;
    while quarantined_path.exists() {
        collision = collision.saturating_add(1);
        quarantined_path = PathBuf::from(format!(
            "{}.corrupt-{timestamp}-{collision}",
            path.display()
        ));
    }

    fs::rename(path, &quarantined_path).map_err(|source| DatabaseError::Quarantine {
        path: path.to_path_buf(),
        source,
    })?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        if sidecar.exists() {
            let quarantined_sidecar =
                PathBuf::from(format!("{}{suffix}", quarantined_path.display()));
            fs::rename(&sidecar, quarantined_sidecar).map_err(|source| {
                DatabaseError::Quarantine {
                    path: sidecar,
                    source,
                }
            })?;
        }
    }
    Ok(quarantined_path)
}

fn search_result_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchResult> {
    let tags_json: String = row.get(2)?;
    let tags = serde_json::from_str(&tags_json).map_err(|source| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(source))
    })?;

    Ok(SearchResult {
        canonical_path: std::path::PathBuf::from(row.get::<_, String>(0)?),
        title: row.get(1)?,
        tags,
        excerpt: row.get(3)?,
        score: row.get(4)?,
    })
}

fn build_fts_query(query: &str) -> Option<String> {
    let terms = query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|term| !term.is_empty())
        .take(MAX_QUERY_TERMS)
        .map(|term| term.chars().take(MAX_QUERY_TERM_CHARS).collect::<String>())
        .map(|term| format!("\"{term}\"*"))
        .collect::<Vec<_>>();

    (!terms.is_empty()).then(|| terms.join(" AND "))
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("failed to create database directory {path}: {source}")]
    CreateDirectory {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to open database at {path}: {source}")]
    Open {
        path: std::path::PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to quarantine unusable database at {path}: {source}")]
    Quarantine {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("database integrity check failed: {0}")]
    Integrity(String),
    #[error("database schema validation failed: {0}")]
    Validation(#[source] rusqlite::Error),
    #[error("database schema version {found} is newer than supported version {supported}")]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("failed to apply database migration {version}: {source}")]
    Migration {
        version: i64,
        #[source]
        source: rusqlite::Error,
    },
    #[error("{0} cannot be represented by SQLite")]
    NumericOverflow(&'static str),
    #[error("path is not valid UTF-8: {0}")]
    PathEncoding(std::path::PathBuf),
    #[error("the system time is before the Unix epoch")]
    InvalidSystemTime,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

impl DatabaseError {
    fn is_recoverable_corruption(&self) -> bool {
        match self {
            Self::Integrity(_) => true,
            Self::Migration { source, .. } | Self::Validation(source) => !matches!(
                source,
                rusqlite::Error::SqliteFailure(error, _)
                    if matches!(
                        error.code,
                        rusqlite::ErrorCode::DatabaseBusy
                            | rusqlite::ErrorCode::DatabaseLocked
                            | rusqlite::ErrorCode::PermissionDenied
                            | rusqlite::ErrorCode::ReadOnly
                            | rusqlite::ErrorCode::SystemIoFailure
                            | rusqlite::ErrorCode::DiskFull
                            | rusqlite::ErrorCode::CannotOpen
                    )
            ),
            Self::Sqlite(rusqlite::Error::SqliteFailure(error, _)) => matches!(
                error.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ),
            _ => false,
        }
    }
}

fn path_as_str(path: &Path) -> Result<&str, DatabaseError> {
    path.to_str()
        .ok_or_else(|| DatabaseError::PathEncoding(path.to_path_buf()))
}

fn unix_time_millis() -> Result<i64, DatabaseError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DatabaseError::InvalidSystemTime)?;
    i64::try_from(duration.as_millis())
        .map_err(|_| DatabaseError::NumericOverflow("Unix timestamp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_latest_schema() {
        let database = IndexDatabase::open_in_memory().expect("open database");

        assert_eq!(
            database.schema_version().expect("schema version"),
            LATEST_SCHEMA_VERSION
        );
        assert!(table_exists(&database.connection, "documents"));
        assert!(table_exists(&database.connection, "documents_fts"));
    }

    #[test]
    fn creates_parent_directories_and_enables_wal_for_a_file_database() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("nested/data/index.sqlite3");

        let database = IndexDatabase::open(&path).expect("open database");
        let journal_mode: String = database
            .connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal mode");

        assert!(path.is_file());
        assert_eq!(journal_mode.to_lowercase(), "wal");
    }

    #[test]
    fn quarantines_a_corrupt_database_and_creates_a_clean_index() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("index.sqlite3");
        fs::write(&path, "not a SQLite database").expect("write corrupt database");

        let (database, recovery) =
            IndexDatabase::open_with_recovery(&path).expect("recover database");
        let recovery = recovery.expect("recovery details");

        assert_eq!(database.document_count().expect("document count"), 0);
        assert!(path.is_file());
        assert!(recovery.quarantined_path.is_file());
        assert_eq!(
            fs::read_to_string(recovery.quarantined_path).expect("quarantined contents"),
            "not a SQLite database"
        );
    }

    #[test]
    fn does_not_replace_an_index_from_a_newer_application() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("index.sqlite3");
        let connection = Connection::open(&path).expect("open database");
        connection
            .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 1)
            .expect("set newer version");
        drop(connection);

        let error = IndexDatabase::open_with_recovery(&path)
            .expect_err("newer schema must not be replaced");

        assert!(matches!(error, DatabaseError::SchemaTooNew { .. }));
        assert!(path.is_file());
    }

    #[test]
    fn migrations_are_idempotent() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("index.sqlite3");

        drop(IndexDatabase::open(&path).expect("first open"));
        let reopened = IndexDatabase::open(&path).expect("second open");

        assert_eq!(
            reopened.schema_version().expect("schema version"),
            LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn upgrades_an_existing_version_one_search_index() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("index.sqlite3");
        let connection = Connection::open(&path).expect("open old database");
        connection
            .execute_batch(MIGRATION_1)
            .expect("version one schema");
        connection
            .pragma_update(None, "user_version", 1)
            .expect("version one marker");
        connection
            .execute(
                "INSERT INTO documents (
                    canonical_path, title, tags_json, body, modified_at_ns,
                    file_size, content_hash, indexed_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    "/notes/old.md",
                    "Old document",
                    "[\"migration\"]",
                    "searchable body",
                    1_i64,
                    10_i64,
                    "hash",
                    1_i64,
                ),
            )
            .expect("old document");
        drop(connection);

        let upgraded = IndexDatabase::open(&path).expect("upgrade database");
        let results = upgraded
            .search("migration", 10)
            .expect("search upgraded index");

        assert_eq!(upgraded.schema_version().expect("schema version"), 2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tags, ["migration"]);
    }

    #[test]
    fn rejects_a_schema_created_by_a_newer_application() {
        let connection = Connection::open_in_memory().expect("open database");
        connection
            .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 1)
            .expect("set schema version");

        let error = IndexDatabase::prepare(connection).expect_err("schema should be rejected");

        assert!(matches!(error, DatabaseError::SchemaTooNew { .. }));
    }

    #[test]
    fn ranks_title_and_tag_matches_above_body_matches() {
        let mut database = IndexDatabase::open_in_memory().expect("open database");
        database
            .upsert_documents(&[
                document(
                    "/notes/title.md",
                    "Docker cleanup",
                    &["containers"],
                    "Remove unused resources",
                ),
                document(
                    "/notes/tag.md",
                    "Container cleanup",
                    &["docker"],
                    "Remove unused resources",
                ),
                document(
                    "/notes/body.md",
                    "Cleanup command",
                    &["containers"],
                    "Run docker system prune",
                ),
            ])
            .expect("index documents");

        let results = database.search("dock", 10).expect("search documents");

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].title, "Docker cleanup");
        assert_eq!(results[1].title, "Container cleanup");
        assert_eq!(results[2].title, "Cleanup command");
    }

    #[test]
    fn identical_documents_do_not_write_the_index_again() {
        let mut database = IndexDatabase::open_in_memory().expect("database");
        let note = document("/notes/note.md", "Note", &[], "body");
        assert_eq!(
            database
                .upsert_documents(std::slice::from_ref(&note))
                .expect("insert"),
            1
        );
        assert_eq!(database.upsert_documents(&[note]).expect("duplicate"), 0);
    }

    #[test]
    fn updating_a_document_replaces_its_search_terms() {
        let mut database = IndexDatabase::open_in_memory().expect("open database");
        let mut note = document("/notes/note.md", "Original", &[], "alpha content");
        database
            .upsert_documents(&[note.clone()])
            .expect("index original document");
        note.title = "Updated".into();
        note.body = "beta content".into();
        note.content_hash = "updated-hash".into();
        database.upsert_documents(&[note]).expect("update document");

        let old_results = database.search("alpha", 10).expect("search old term");
        let new_results = database.search("beta", 10).expect("search new term");

        assert!(old_results.is_empty());
        assert_eq!(new_results.len(), 1);
        assert_eq!(new_results[0].title, "Updated");
    }

    #[test]
    fn empty_queries_return_a_bounded_alphabetical_list() {
        let mut database = IndexDatabase::open_in_memory().expect("open database");
        database
            .upsert_documents(&[
                document("/notes/z.md", "Zulu", &[], "last"),
                document("/notes/a.md", "Alpha", &[], "first"),
            ])
            .expect("index documents");

        let results = database.search("  ", 1).expect("list documents");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Alpha");
    }

    #[test]
    fn punctuation_only_queries_do_not_expose_fts_syntax() {
        let mut database = IndexDatabase::open_in_memory().expect("open database");
        database
            .upsert_documents(&[document("/notes/a.md", "Alpha", &[], "body")])
            .expect("index document");

        let results = database.search("\"* OR --", 10).expect("safe search");

        assert!(results.is_empty());
    }

    #[test]
    fn reconciliation_removes_missing_files_after_an_incomplete_scan() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("removed.md");
        std::fs::write(&path, "body").expect("write document");
        let canonical_path = path.canonicalize().expect("canonical path");
        let mut database = IndexDatabase::open_in_memory().expect("open database");
        database
            .upsert_documents(&[document(
                canonical_path.to_str().expect("UTF-8 path"),
                "Removed",
                &[],
                "body",
            )])
            .expect("index document");
        std::fs::remove_file(path).expect("remove document");

        let removed = database
            .remove_missing_documents(&[], &HashSet::new())
            .expect("reconcile documents");

        assert_eq!(removed, 1);
    }

    #[test]
    fn clearing_documents_also_clears_full_text_results() {
        let mut database = IndexDatabase::open_in_memory().expect("open database");
        database
            .upsert_documents(&[document("/notes/a.md", "Alpha", &[], "searchable")])
            .expect("index document");

        let removed = database.clear_documents().expect("clear documents");

        assert_eq!(removed, 1);
        assert!(
            database
                .search("searchable", 10)
                .expect("search")
                .is_empty()
        );
    }

    #[test]
    fn document_triggers_keep_the_fts_table_searchable() {
        let database = IndexDatabase::open_in_memory().expect("open database");
        database
            .connection
            .execute(
                "INSERT INTO documents (
                    canonical_path, title, tags_json, body, modified_at_ns,
                    file_size, content_hash, indexed_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    "/notes/rust.md",
                    "Rust async",
                    "[\"rust\"]",
                    "Tokio task",
                    1_i64,
                    10_i64,
                    "hash",
                    1_i64,
                ),
            )
            .expect("insert document");

        let count: i64 = database
            .connection
            .query_row(
                "SELECT count(*) FROM documents_fts WHERE documents_fts MATCH 'tokio'",
                [],
                |row| row.get(0),
            )
            .expect("search FTS");

        assert_eq!(count, 1);
    }

    fn document(path: &str, title: &str, tags: &[&str], body: &str) -> Document {
        Document {
            canonical_path: path.into(),
            metadata_id: None,
            title: title.into(),
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            body: body.into(),
            modified_at_ns: 1,
            file_size: u64::try_from(body.len()).expect("body size"),
            content_hash: format!("hash-{path}"),
        }
    }

    fn table_exists(connection: &Connection, name: &str) -> bool {
        connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                    WHERE name = ?1 AND type IN ('table', 'view')
                )",
                [name],
                |row| row.get(0),
            )
            .expect("query schema")
    }
}
