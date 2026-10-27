use std::{collections::HashSet, mem, path::PathBuf};

use thiserror::Error;

use crate::config::ScanConfig;

use super::{
    database::{DatabaseError, IndexDatabase},
    document::Document,
    scanner::{MarkdownScanner, ScanSummary, ScannerError},
};

const DEFAULT_BATCH_SIZE: usize = 100;
const MAX_RETAINED_PARSE_ISSUES: usize = 100;

pub struct MarkdownIndexer<'a> {
    config: &'a ScanConfig,
    database: &'a mut IndexDatabase,
    batch_size: usize,
}

impl<'a> MarkdownIndexer<'a> {
    pub fn new(config: &'a ScanConfig, database: &'a mut IndexDatabase) -> Self {
        Self {
            config,
            database,
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }

    pub fn run(&mut self) -> Result<IndexingReport, IndexingError> {
        self.run_with_control(|| false, |_| {})
    }

    pub fn run_with_control(
        &mut self,
        mut is_cancelled: impl FnMut() -> bool,
        mut on_progress: impl FnMut(IndexingProgress),
    ) -> Result<IndexingReport, IndexingError> {
        let scanner = MarkdownScanner::new(self.config)?;
        let mut batch = Vec::with_capacity(self.batch_size);
        let mut indexed_files = 0;
        let mut removed_files = 0;
        let mut unchanged_files = 0;
        let mut parse_issue_count = 0;
        let mut parse_issues = Vec::new();
        let mut observed_paths = HashSet::new();
        let mut processed_files = 0;

        let scan = scanner.visit_with_control(
            |file| -> Result<(), IndexingError> {
                processed_files += 1;
                if self.database.is_current(&file)? {
                    observed_paths.insert(file.canonical_path);
                    unchanged_files += 1;
                    on_progress(IndexingProgress {
                        processed_files,
                        indexed_files,
                        unchanged_files,
                        issue_count: parse_issue_count,
                    });
                    return Ok(());
                }

                match Document::parse(&file, self.config.max_file_size_bytes) {
                    Ok(document) => {
                        observed_paths.insert(file.canonical_path);
                        batch.push(document);
                        if batch.len() >= self.batch_size {
                            let pending = mem::take(&mut batch);
                            indexed_files += self.database.upsert_documents(&pending)?;
                        }
                    }
                    Err(error) => {
                        if error.invalidates_cached_content() {
                            removed_files += self.database.remove_document(&file.canonical_path)?;
                        } else {
                            observed_paths.insert(file.canonical_path.clone());
                        }
                        parse_issue_count += 1;
                        if parse_issues.len() < MAX_RETAINED_PARSE_ISSUES {
                            parse_issues.push(IndexingIssue {
                                path: file.canonical_path,
                                message: error.to_string(),
                            });
                        }
                    }
                }

                on_progress(IndexingProgress {
                    processed_files,
                    indexed_files,
                    unchanged_files,
                    issue_count: parse_issue_count,
                });
                Ok(())
            },
            &mut is_cancelled,
        )?;

        if scan.cancelled || is_cancelled() {
            return Err(IndexingError::Cancelled);
        }

        if !batch.is_empty() {
            indexed_files += self.database.upsert_documents(&batch)?;
        }

        removed_files += self
            .database
            .remove_missing_documents(&scan.completed_roots, &observed_paths)?;
        // An unavailable root must not make its cached documents look out of scope.
        if let Ok(roots) = self
            .config
            .roots
            .iter()
            .map(std::fs::canonicalize)
            .collect::<Result<Vec<_>, _>>()
        {
            removed_files += self.database.remove_documents_outside_roots(&roots)?;
        }
        on_progress(IndexingProgress {
            processed_files,
            indexed_files,
            unchanged_files,
            issue_count: parse_issue_count + scan.issue_count,
        });

        Ok(IndexingReport {
            scan,
            indexed_files,
            unchanged_files,
            removed_files,
            parse_issue_count,
            parse_issues,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexingProgress {
    pub processed_files: usize,
    pub indexed_files: usize,
    pub unchanged_files: usize,
    pub issue_count: usize,
}

#[derive(Debug)]
pub struct IndexingReport {
    pub scan: ScanSummary,
    pub indexed_files: usize,
    pub unchanged_files: usize,
    pub removed_files: usize,
    pub parse_issue_count: usize,
    pub parse_issues: Vec<IndexingIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexingIssue {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum IndexingError {
    #[error("indexing was cancelled")]
    Cancelled,
    #[error(transparent)]
    Scanner(#[from] ScannerError),
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn indexes_valid_markdown_and_skips_unchanged_files() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        fs::write(
            temporary_directory.path().join("rust.md"),
            "---\ntitle: Rust note\ntags: [rust]\n---\nBorrow safely\n",
        )
        .expect("write Markdown");
        fs::write(
            temporary_directory.path().join("invalid.md"),
            "---\ntitle: Missing delimiter\n",
        )
        .expect("write invalid Markdown");
        fs::write(temporary_directory.path().join("ignored.txt"), "ignored").expect("write text");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let mut database = IndexDatabase::open_in_memory().expect("database");

        let first = MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("first index");
        let second = MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("second index");

        assert_eq!(database.document_count().expect("document count"), 1);
        assert_eq!(first.indexed_files, 1);
        assert_eq!(first.parse_issue_count, 1);
        assert_eq!(second.indexed_files, 0);
        assert_eq!(second.unchanged_files, 1);
        assert_eq!(second.parse_issue_count, 1);
    }

    #[test]
    fn removes_documents_that_disappear_after_a_complete_scan() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("note.md");
        fs::write(&path, "temporary").expect("write Markdown");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let mut database = IndexDatabase::open_in_memory().expect("database");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("initial index");
        fs::remove_file(&path).expect("remove Markdown");

        let report = MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("reconcile index");

        assert_eq!(report.removed_files, 1);
        assert_eq!(database.document_count().expect("document count"), 0);
    }

    #[test]
    fn removes_existing_documents_from_roots_removed_from_configuration() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let old_root = temporary_directory.path().join("old");
        let new_root = temporary_directory.path().join("new");
        fs::create_dir(&old_root).expect("create old root");
        fs::create_dir(&new_root).expect("create new root");
        fs::write(old_root.join("old.md"), "oldroot body").expect("write old note");
        fs::write(new_root.join("new.md"), "newroot body").expect("write new note");
        let mut database = IndexDatabase::open_in_memory().expect("database");
        MarkdownIndexer::new(&ScanConfig::default_for_home(&old_root), &mut database)
            .run()
            .expect("initial index");

        let report = MarkdownIndexer::new(&ScanConfig::default_for_home(&new_root), &mut database)
            .run()
            .expect("reconcile changed roots");

        assert_eq!(report.removed_files, 1);
        assert!(old_root.join("old.md").is_file());
        assert!(
            database
                .search("oldroot", 10)
                .expect("old search")
                .is_empty()
        );
        assert_eq!(database.search("newroot", 10).expect("new search").len(), 1);
    }

    #[test]
    fn defers_scope_cleanup_when_a_configured_root_cannot_be_resolved() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let available_root = temporary_directory.path().join("available");
        fs::create_dir(&available_root).expect("create available root");
        fs::write(available_root.join("note.md"), "cached body").expect("write note");
        let mut database = IndexDatabase::open_in_memory().expect("database");
        MarkdownIndexer::new(
            &ScanConfig::default_for_home(&available_root),
            &mut database,
        )
        .run()
        .expect("initial index");
        let unavailable_root = temporary_directory.path().join("unavailable");

        let report = MarkdownIndexer::new(
            &ScanConfig::default_for_home(&unavailable_root),
            &mut database,
        )
        .run()
        .expect("incomplete reconciliation");

        assert_eq!(report.removed_files, 0);
        assert_eq!(report.scan.issue_count, 1);
        assert_eq!(
            database.search("cached", 10).expect("cached search").len(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn incomplete_scans_remove_known_malformed_content() {
        use std::os::unix::fs::symlink;
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path();
        let invalid = root.join("invalid.md");
        fs::write(&invalid, "previouscontent").expect("original note");
        fs::write(root.join("valid.md"), "preservedcontent").expect("valid note");
        let mut config = ScanConfig::default_for_home(root);
        config.follow_symbolic_links = true;
        let mut database = IndexDatabase::open_in_memory().expect("database");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("initial index");
        fs::write(&invalid, "---\ntitle: missing delimiter\n").expect("malformed note");
        // A dangling link makes traversal incomplete regardless of user privileges.
        symlink(root.join("missing"), root.join("broken")).expect("dangling link");
        let report = MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("reconcile");
        assert!(report.scan.completed_roots.is_empty());
        assert_eq!(report.scan.issue_count, 1);
        assert_eq!(report.parse_issue_count, 1);
        assert_eq!(report.removed_files, 1);
        assert!(
            database
                .search("previouscontent", 10)
                .expect("stale search")
                .is_empty()
        );
        assert_eq!(
            database
                .search("preservedcontent", 10)
                .expect("valid search")
                .len(),
            1
        );
        assert!(invalid.is_file());
    }

    #[test]
    fn reports_progress_and_supports_cancellation() {
        use std::cell::Cell;

        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        fs::write(temporary_directory.path().join("one.md"), "one").expect("write Markdown");
        fs::write(temporary_directory.path().join("two.md"), "two").expect("write Markdown");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let mut database = IndexDatabase::open_in_memory().expect("database");
        let cancellation_checks = Cell::new(0);
        let mut progress_updates = Vec::new();

        let error = MarkdownIndexer::new(&config, &mut database)
            .run_with_control(
                || {
                    let checks = cancellation_checks.get();
                    cancellation_checks.set(checks + 1);
                    checks > 1
                },
                |progress| progress_updates.push(progress),
            )
            .expect_err("indexing should be cancelled");

        assert!(matches!(error, IndexingError::Cancelled));
        assert_eq!(progress_updates.len(), 1);
        assert_eq!(progress_updates[0].processed_files, 1);
    }

    #[test]
    fn updates_a_document_after_its_file_changes() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("note.md");
        fs::write(&path, "short").expect("write Markdown");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let mut database = IndexDatabase::open_in_memory().expect("database");

        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("initial index");
        fs::write(&path, "a changed and longer body").expect("update Markdown");
        let report = MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("updated index");

        assert_eq!(report.indexed_files, 1);
        assert_eq!(report.unchanged_files, 0);
        assert_eq!(database.document_count().expect("document count"), 1);
    }
}
