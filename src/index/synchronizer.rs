use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;

use crate::config::ScanConfig;

use super::{
    database::{DatabaseError, IndexDatabase},
    document::Document,
    indexer::{IndexingError, MarkdownIndexer},
    scanner::{MarkdownScanner, ScanIssue, ScanIssueKind, ScannerError},
};

const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(250);
const MAX_RETAINED_ISSUES: usize = 100;
const WATCH_LOOP_INTERVAL: Duration = Duration::from_millis(500);
const WATCH_RETRY_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub struct WatchBatch {
    pub paths: Vec<PathBuf>,
    pub needs_reconciliation: bool,
}

pub struct FilesystemWatcher {
    _watcher: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
    debounce: Duration,
    unwatched_roots: Vec<PathBuf>,
    root_error: Option<String>,
}

impl FilesystemWatcher {
    pub fn new(config: &ScanConfig) -> Result<Self, WatchError> {
        let (sender, receiver) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;
        let mut watcher = Self {
            _watcher: watcher,
            receiver,
            debounce: DEFAULT_DEBOUNCE,
            unwatched_roots: Vec::new(),
            root_error: None,
        };
        watcher.watch_roots(config.roots.clone());
        Ok(watcher)
    }

    fn watch_roots(&mut self, roots: Vec<PathBuf>) -> usize {
        let mut connected = 0;
        self.root_error = None;
        for root in roots {
            match self._watcher.watch(&root, RecursiveMode::Recursive) {
                Ok(()) => connected += 1,
                Err(source) => {
                    self.root_error = Some(
                        WatchError::WatchRoot {
                            path: root.clone(),
                            source,
                        }
                        .to_string(),
                    );
                    self.unwatched_roots.push(root);
                }
            }
        }
        connected
    }

    fn retry_unavailable_roots(&mut self) -> usize {
        let roots = std::mem::take(&mut self.unwatched_roots);
        self.watch_roots(roots)
    }

    pub fn next_batch(&self, initial_wait: Duration) -> Result<WatchBatch, WatchError> {
        let first = match self.receiver.recv_timeout(initial_wait) {
            Ok(event) => event?,
            Err(RecvTimeoutError::Timeout) => return Ok(WatchBatch::default()),
            Err(RecvTimeoutError::Disconnected) => return Err(WatchError::Disconnected),
        };
        let mut paths = HashSet::new();
        let mut needs_reconciliation = first.need_rescan();
        collect_event_paths(first, &mut paths);

        let deadline = Instant::now() + self.debounce;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match self.receiver.recv_timeout(remaining) {
                Ok(event) => {
                    let event = event?;
                    needs_reconciliation |= event.need_rescan();
                    collect_event_paths(event, &mut paths);
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return Err(WatchError::Disconnected),
            }
        }

        let mut paths = paths.into_iter().collect::<Vec<_>>();
        paths.sort();
        Ok(WatchBatch {
            paths,
            needs_reconciliation,
        })
    }
}

fn collect_event_paths(event: Event, paths: &mut HashSet<PathBuf>) {
    if !matches!(event.kind, EventKind::Access(_)) {
        paths.extend(event.paths);
    }
}

fn normalize_missing_path(path: &std::path::Path) -> PathBuf {
    let mut existing_ancestor = path;
    let mut missing_components = Vec::new();
    while !existing_ancestor.exists() {
        let Some(file_name) = existing_ancestor.file_name() else {
            return path.to_path_buf();
        };
        missing_components.push(file_name.to_os_string());
        let Some(parent) = existing_ancestor.parent() else {
            return path.to_path_buf();
        };
        existing_ancestor = parent;
    }

    let Ok(mut normalized) = existing_ancestor.canonicalize() else {
        return path.to_path_buf();
    };
    for component in missing_components.into_iter().rev() {
        normalized.push(component);
    }
    normalized
}

pub struct IndexSynchronizer<'a> {
    config: &'a ScanConfig,
    database: &'a mut IndexDatabase,
}

impl<'a> IndexSynchronizer<'a> {
    pub fn new(config: &'a ScanConfig, database: &'a mut IndexDatabase) -> Self {
        Self { config, database }
    }

    pub fn synchronize_paths(
        &mut self,
        changed_paths: &[PathBuf],
    ) -> Result<SynchronizationReport, SynchronizationError> {
        self.synchronize_paths_with_control(changed_paths, || false)
    }

    pub fn synchronize_paths_with_control(
        &mut self,
        changed_paths: &[PathBuf],
        mut is_cancelled: impl FnMut() -> bool,
    ) -> Result<SynchronizationReport, SynchronizationError> {
        let scanner = MarkdownScanner::new(self.config)?;
        let mut report = SynchronizationReport::default();
        let mut unique_paths = changed_paths.iter().collect::<HashSet<_>>();
        let mut unique_paths = unique_paths.drain().cloned().collect::<Vec<_>>();
        unique_paths.sort();

        for path in unique_paths {
            if is_cancelled() {
                return Err(SynchronizationError::Cancelled);
            }
            if path.is_dir() {
                self.synchronize_directory(&scanner, &path, &mut report, &mut is_cancelled)?;
                continue;
            }
            if !path.exists() {
                let normalized_path = normalize_missing_path(&path);
                report.removed_files += self.database.remove_paths_under(&normalized_path)?;
                continue;
            }

            match scanner.discover_file(&path) {
                Ok(Some(file)) => match Document::parse(&file, self.config.max_file_size_bytes) {
                    Ok(document) => {
                        let updated = self.database.upsert_documents(&[document])?;
                        report.indexed_files += updated;
                        report.unchanged_files += usize::from(updated == 0);
                    }
                    Err(error) => {
                        if error.invalidates_cached_content() {
                            report.removed_files +=
                                self.database.remove_document(&file.canonical_path)?;
                        }
                        report.record_issue(file.canonical_path, error.to_string());
                    }
                },
                Ok(None) => {
                    let stored_path = path.canonicalize().unwrap_or(path);
                    report.removed_files += self.database.remove_paths_under(&stored_path)?;
                }
                Err(issue) if issue.kind == ScanIssueKind::FileTooLarge => {
                    report.removed_files += self.database.remove_paths_under(&issue.path)?;
                    report.record_scan_issue(issue);
                }
                Err(issue) => report.record_scan_issue(issue),
            }
        }

        Ok(report)
    }

    fn synchronize_directory(
        &mut self,
        scanner: &MarkdownScanner<'_>,
        directory: &std::path::Path,
        report: &mut SynchronizationReport,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), SynchronizationError> {
        const BATCH_SIZE: usize = 100;

        let mut observed_paths = HashSet::new();
        let mut documents = Vec::with_capacity(BATCH_SIZE);
        let scan = scanner.visit_subtree_with_control(
            directory,
            |file| -> Result<(), DatabaseError> {
                if self.database.is_current(&file)? {
                    observed_paths.insert(file.canonical_path);
                    report.unchanged_files += 1;
                    return Ok(());
                }

                match Document::parse(&file, self.config.max_file_size_bytes) {
                    Ok(document) => {
                        observed_paths.insert(file.canonical_path);
                        documents.push(document);
                        if documents.len() >= BATCH_SIZE {
                            report.indexed_files += self.database.upsert_documents(&documents)?;
                            documents.clear();
                        }
                    }
                    Err(error) => {
                        if error.invalidates_cached_content() {
                            report.removed_files +=
                                self.database.remove_document(&file.canonical_path)?;
                        } else {
                            observed_paths.insert(file.canonical_path.clone());
                        }
                        report.record_issue(file.canonical_path, error.to_string());
                    }
                }
                Ok(())
            },
            &mut *is_cancelled,
        )?;

        if scan.cancelled || is_cancelled() {
            return Err(SynchronizationError::Cancelled);
        }
        if !documents.is_empty() {
            report.indexed_files += self.database.upsert_documents(&documents)?;
        }
        report.removed_files += self
            .database
            .remove_missing_documents(&scan.completed_roots, &observed_paths)?;
        report.reconciled = true;
        for issue in scan.issues {
            report.record_scan_issue(issue);
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct SynchronizationReport {
    pub indexed_files: usize,
    pub unchanged_files: usize,
    pub removed_files: usize,
    pub reconciled: bool,
    pub issue_count: usize,
    pub issues: Vec<SynchronizationIssue>,
}

impl SynchronizationReport {
    fn record_scan_issue(&mut self, issue: ScanIssue) {
        self.record_issue(issue.path, issue.message);
    }

    fn record_issue(&mut self, path: PathBuf, message: String) {
        self.issue_count += 1;
        if self.issues.len() < MAX_RETAINED_ISSUES {
            self.issues.push(SynchronizationIssue { path, message });
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynchronizationIssue {
    pub path: PathBuf,
    pub message: String,
}

pub struct IndexWatchService {
    shutdown: Arc<AtomicBool>,
    rebuild_requested: Arc<AtomicBool>,
    requested_paths: Arc<Mutex<Vec<PathBuf>>>,
    status: Arc<Mutex<WatchServiceStatus>>,
    thread: Option<JoinHandle<()>>,
}

impl IndexWatchService {
    pub fn start(config: ScanConfig, database_path: PathBuf) -> Result<Self, WatchServiceError> {
        let (mut database, recovery) = IndexDatabase::open_with_recovery(&database_path)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let rebuild_requested = Arc::new(AtomicBool::new(false));
        let requested_paths = Arc::new(Mutex::new(Vec::new()));
        let worker_requested_paths = Arc::clone(&requested_paths);
        let status = Arc::new(Mutex::new(WatchServiceStatus {
            database_recovery: recovery.map(|recovery| recovery.quarantined_path),
            ..WatchServiceStatus::default()
        }));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_rebuild_requested = Arc::clone(&rebuild_requested);
        let worker_status = Arc::clone(&status);
        let thread = thread::Builder::new()
            .name("markdown-index-watcher".into())
            .spawn(move || {
                let mut watcher = create_watcher(&config, &worker_status);
                run_full_index(
                    &config,
                    &mut database,
                    &worker_shutdown,
                    &worker_status,
                    IndexActivity::InitialScan,
                );
                let mut last_watch_retry = Instant::now();

                while !worker_shutdown.load(Ordering::Acquire) {
                    let paths = std::mem::take(
                        &mut *worker_requested_paths
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()),
                    );
                    if !paths.is_empty() {
                        process_watch_batch(
                            &config,
                            &mut database,
                            &worker_shutdown,
                            &worker_status,
                            WatchBatch {
                                paths,
                                needs_reconciliation: false,
                            },
                        );
                    }
                    if worker_rebuild_requested.swap(false, Ordering::AcqRel) {
                        update_watch_status(&worker_status, |status| {
                            status.activity = IndexActivity::Rebuilding;
                            status.rebuild_in_progress = true;
                            status.progress = Default::default();
                        });
                        match database.clear_documents() {
                            Ok(removed) => {
                                if removed > 0 {
                                    update_watch_status(&worker_status, |status| {
                                        status.revision = status.revision.wrapping_add(1);
                                    });
                                }
                                run_full_index(
                                    &config,
                                    &mut database,
                                    &worker_shutdown,
                                    &worker_status,
                                    IndexActivity::Rebuilding,
                                );
                            }
                            Err(error) => record_service_error(
                                &worker_status,
                                "index rebuild",
                                error.to_string(),
                            ),
                        }
                        update_watch_status(&worker_status, |status| {
                            status.rebuild_in_progress = false;
                            if !worker_shutdown.load(Ordering::Acquire) {
                                status.activity = IndexActivity::Watching;
                            }
                        });
                    }

                    if worker_shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    if watcher.is_none() {
                        if wait_interruptibly(&worker_shutdown, WATCH_RETRY_INTERVAL) {
                            break;
                        }
                        watcher = create_watcher(&config, &worker_status);
                        if watcher.is_some() {
                            run_full_index(
                                &config,
                                &mut database,
                                &worker_shutdown,
                                &worker_status,
                                IndexActivity::Reconciling,
                            );
                        }
                        continue;
                    }

                    if let Some(watcher) = watcher.as_mut()
                        && !watcher.unwatched_roots.is_empty()
                        && last_watch_retry.elapsed() >= WATCH_RETRY_INTERVAL
                    {
                        let connected = watcher.retry_unavailable_roots();
                        update_watcher_status(watcher, &worker_status);
                        last_watch_retry = Instant::now();
                        if connected > 0 {
                            run_full_index(
                                &config,
                                &mut database,
                                &worker_shutdown,
                                &worker_status,
                                IndexActivity::Reconciling,
                            );
                        }
                    }
                    if worker_shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    let watch_result = watcher
                        .as_ref()
                        .expect("watcher was checked above")
                        .next_batch(WATCH_LOOP_INTERVAL);
                    match watch_result {
                        Ok(batch) => process_watch_batch(
                            &config,
                            &mut database,
                            &worker_shutdown,
                            &worker_status,
                            batch,
                        ),
                        Err(error) => {
                            watcher = None;
                            update_watch_status(&worker_status, |status| {
                                status.watcher_connected = false;
                                status.watcher_restart_count =
                                    status.watcher_restart_count.saturating_add(1);
                            });
                            record_service_error(
                                &worker_status,
                                "filesystem watcher",
                                error.to_string(),
                            );
                        }
                    }
                }

                update_watch_status(&worker_status, |status| {
                    status.activity = IndexActivity::Stopping;
                    status.watcher_connected = false;
                    status.rebuild_in_progress = false;
                });
            })
            .map_err(WatchServiceError::Spawn)?;

        Ok(Self {
            shutdown,
            rebuild_requested,
            requested_paths,
            status,
            thread: Some(thread),
        })
    }

    pub fn request_sync(&self, path: PathBuf) {
        let mut paths = self
            .requested_paths
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !paths.contains(&path) {
            paths.push(path);
        }
    }

    pub fn request_rebuild(&self) {
        self.rebuild_requested.store(true, Ordering::Release);
    }

    pub fn status(&self) -> WatchServiceStatus {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Drop for IndexWatchService {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn create_watcher(
    config: &ScanConfig,
    status: &Mutex<WatchServiceStatus>,
) -> Option<FilesystemWatcher> {
    match FilesystemWatcher::new(config) {
        Ok(watcher) => {
            update_watcher_status(&watcher, status);
            Some(watcher)
        }
        Err(error) => {
            update_watch_status(status, |status| {
                status.watcher_connected = false;
                status.watcher_restart_count = status.watcher_restart_count.saturating_add(1);
            });
            record_service_error(status, "filesystem watcher", error.to_string());
            None
        }
    }
}

fn update_watcher_status(watcher: &FilesystemWatcher, status: &Mutex<WatchServiceStatus>) {
    update_watch_status(status, |status| {
        status.watcher_connected = watcher.unwatched_roots.is_empty();
        if !status.watcher_connected {
            status.watcher_restart_count = status.watcher_restart_count.saturating_add(1);
        }
        status.last_error = watcher
            .root_error
            .as_ref()
            .map(|error| format!("filesystem watcher: {error}"));
    });
}

fn process_watch_batch(
    config: &ScanConfig,
    database: &mut IndexDatabase,
    shutdown: &AtomicBool,
    status: &Mutex<WatchServiceStatus>,
    batch: WatchBatch,
) {
    if shutdown.load(Ordering::Acquire) {
        return;
    }
    if batch.needs_reconciliation {
        run_full_index(
            config,
            database,
            shutdown,
            status,
            IndexActivity::Reconciling,
        );
    } else if !batch.paths.is_empty() {
        match IndexSynchronizer::new(config, database)
            .synchronize_paths_with_control(&batch.paths, || shutdown.load(Ordering::Acquire))
        {
            Ok(report) => update_watch_status(status, |status| {
                apply_synchronization_report(status, report);
            }),
            Err(SynchronizationError::Cancelled) if shutdown.load(Ordering::Acquire) => {}
            Err(error) => {
                record_service_error(status, "filesystem synchronization", error.to_string())
            }
        }
    }
}

fn run_full_index(
    config: &ScanConfig,
    database: &mut IndexDatabase,
    shutdown: &AtomicBool,
    status: &Mutex<WatchServiceStatus>,
    activity: IndexActivity,
) {
    update_watch_status(status, |status| {
        status.activity = activity;
        status.progress = Default::default();
        if status.watcher_connected {
            status.last_error = None;
        }
    });
    let result = MarkdownIndexer::new(config, database).run_with_control(
        || shutdown.load(Ordering::Acquire),
        |progress| update_watch_status(status, |status| status.progress = progress),
    );

    match result {
        Ok(report) => update_watch_status(status, |status| {
            apply_indexing_report(status, report);
            status.initial_scan_complete = true;
            status.activity = IndexActivity::Watching;
        }),
        Err(IndexingError::Cancelled) if shutdown.load(Ordering::Acquire) => {}
        Err(error) => record_service_error(status, "index reconciliation", error.to_string()),
    }
}

fn apply_indexing_report(status: &mut WatchServiceStatus, report: super::indexer::IndexingReport) {
    status.progress.processed_files = report.scan.eligible_files;
    status.progress.indexed_files = report.indexed_files;
    status.progress.unchanged_files = report.unchanged_files;
    status.progress.issue_count = report.scan.issue_count + report.parse_issue_count;
    status.removed_files = report.removed_files;
    status.issue_count = status.progress.issue_count;
    status.recent_issues.clear();
    for issue in report.scan.issues {
        status.push_issue(issue.path, issue.message);
    }
    for issue in report.parse_issues {
        status.push_issue(issue.path, issue.message);
    }
    if report.indexed_files > 0 || report.removed_files > 0 {
        status.revision = status.revision.wrapping_add(1);
    }
}

fn apply_synchronization_report(status: &mut WatchServiceStatus, report: SynchronizationReport) {
    status.progress.processed_files = report.indexed_files + report.unchanged_files;
    status.progress.indexed_files = report.indexed_files;
    status.progress.unchanged_files = report.unchanged_files;
    status.progress.issue_count = report.issue_count;
    status.removed_files = report.removed_files;
    status.issue_count = report.issue_count;
    status.recent_issues.clear();
    for issue in report.issues {
        status.push_issue(issue.path, issue.message);
    }
    if status.watcher_connected {
        status.last_error = None;
    }
    if report.indexed_files > 0 || report.removed_files > 0 {
        status.revision = status.revision.wrapping_add(1);
    }
}

fn record_service_error(status: &Mutex<WatchServiceStatus>, operation: &str, message: String) {
    update_watch_status(status, |status| {
        status.last_error = Some(format!("{operation}: {message}"));
        status.issue_count = status.issue_count.saturating_add(1);
    });
}

fn wait_interruptibly(shutdown: &AtomicBool, duration: Duration) -> bool {
    const SLEEP_SLICE: Duration = Duration::from_millis(25);
    let deadline = Instant::now() + duration;
    while !shutdown.load(Ordering::Acquire) {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        thread::sleep(remaining.min(SLEEP_SLICE));
    }
    true
}

fn update_watch_status(
    status: &Mutex<WatchServiceStatus>,
    update: impl FnOnce(&mut WatchServiceStatus),
) {
    let mut status = status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    update(&mut status);
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IndexActivity {
    #[default]
    InitialScan,
    Reconciling,
    Watching,
    Rebuilding,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDiagnostic {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchServiceStatus {
    pub activity: IndexActivity,
    pub initial_scan_complete: bool,
    pub rebuild_in_progress: bool,
    pub progress: super::indexer::IndexingProgress,
    pub removed_files: usize,
    pub issue_count: usize,
    pub recent_issues: Vec<IndexDiagnostic>,
    pub watcher_connected: bool,
    pub watcher_restart_count: usize,
    pub database_recovery: Option<PathBuf>,
    pub revision: u64,
    pub last_error: Option<String>,
}

impl WatchServiceStatus {
    fn push_issue(&mut self, path: PathBuf, message: String) {
        if self.recent_issues.len() < MAX_RETAINED_ISSUES {
            self.recent_issues.push(IndexDiagnostic { path, message });
        }
    }
}

#[derive(Debug, Error)]
pub enum WatchServiceError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error("failed to start the index watcher thread: {0}")]
    Spawn(#[source] std::io::Error),
}

#[derive(Debug, Error)]
pub enum WatchError {
    #[error(transparent)]
    Notify(#[from] notify::Error),
    #[error("failed to watch {path}: {source}")]
    WatchRoot {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
    #[error("the filesystem watcher event channel disconnected")]
    Disconnected,
}

#[derive(Debug, Error)]
pub enum SynchronizationError {
    #[error("filesystem synchronization was cancelled")]
    Cancelled,
    #[error(transparent)]
    Scanner(#[from] ScannerError),
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Indexing(#[from] IndexingError),
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn incrementally_updates_and_removes_files() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("note.md");
        fs::write(&path, "initial body").expect("write Markdown");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let mut database = IndexDatabase::open_in_memory().expect("database");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("initial index");
        fs::write(&path, "updated searchable body").expect("update Markdown");

        let updated = IndexSynchronizer::new(&config, &mut database)
            .synchronize_paths(std::slice::from_ref(&path))
            .expect("synchronize update");
        let results = database.search("searchable", 10).expect("search update");
        fs::remove_file(&path).expect("remove Markdown");
        let removed = IndexSynchronizer::new(&config, &mut database)
            .synchronize_paths(std::slice::from_ref(&path))
            .expect("synchronize removal");

        assert_eq!(updated.indexed_files, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(removed.removed_files, 1);
        assert_eq!(database.document_count().expect("document count"), 0);
    }

    #[test]
    fn removes_stale_content_when_a_changed_file_no_longer_parses() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("note.md");
        fs::write(&path, "previously searchable").expect("write Markdown");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let mut database = IndexDatabase::open_in_memory().expect("database");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("initial index");
        fs::write(&path, "---\ntitle: invalid").expect("write invalid Markdown");

        let report = IndexSynchronizer::new(&config, &mut database)
            .synchronize_paths(std::slice::from_ref(&path))
            .expect("synchronize invalid file");

        assert_eq!(report.removed_files, 1);
        assert_eq!(report.issue_count, 1);
        assert!(
            database
                .search("previously", 10)
                .expect("search")
                .is_empty()
        );
    }

    #[test]
    fn unchanged_and_irrelevant_events_do_not_advance_search_revision() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path();
        let note = root.join("note.md");
        fs::write(&note, "body").expect("note");
        let config = ScanConfig::default_for_home(root);
        let mut database = IndexDatabase::open_in_memory().expect("database");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("index");
        let status = Mutex::new(WatchServiceStatus::default());
        for paths in [
            vec![note],
            vec![root.join("irrelevant.txt")],
            vec![root.to_path_buf()],
        ] {
            process_watch_batch(
                &config,
                &mut database,
                &AtomicBool::new(false),
                &status,
                WatchBatch {
                    paths,
                    needs_reconciliation: false,
                },
            );
        }
        process_watch_batch(
            &config,
            &mut database,
            &AtomicBool::new(false),
            &status,
            WatchBatch {
                paths: vec![],
                needs_reconciliation: true,
            },
        );
        assert_eq!(status.lock().expect("status").revision, 0);
        fs::write(root.join("new.md"), "new body").expect("new note");
        process_watch_batch(
            &config,
            &mut database,
            &AtomicBool::new(false),
            &status,
            WatchBatch {
                paths: vec![root.join("new.md")],
                needs_reconciliation: false,
            },
        );
        assert_eq!(status.lock().expect("status").revision, 1);
    }

    #[test]
    fn directory_events_trigger_a_safe_reconciliation() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let directory = temporary_directory.path().join("new-directory");
        fs::create_dir(&directory).expect("create directory");
        fs::write(directory.join("note.md"), "new body").expect("write Markdown");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let mut database = IndexDatabase::open_in_memory().expect("database");

        let report = IndexSynchronizer::new(&config, &mut database)
            .synchronize_paths(std::slice::from_ref(&directory))
            .expect("synchronize directory");

        assert!(report.reconciled);
        assert_eq!(report.indexed_files, 1);
        assert_eq!(database.document_count().expect("document count"), 1);
    }

    #[test]
    fn directory_events_reconcile_only_the_changed_subtree() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let selected = temporary_directory.path().join("selected");
        let other = temporary_directory.path().join("other");
        fs::create_dir(&selected).expect("create selected directory");
        fs::create_dir(&other).expect("create other directory");
        fs::write(selected.join("selected.md"), "selected body").expect("write Markdown");
        fs::write(other.join("other.md"), "old outside body").expect("write Markdown");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let mut database = IndexDatabase::open_in_memory().expect("database");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("initial index");
        fs::write(other.join("other.md"), "new outside body").expect("change outside file");

        IndexSynchronizer::new(&config, &mut database)
            .synchronize_paths(std::slice::from_ref(&selected))
            .expect("synchronize selected subtree");

        assert_eq!(database.search("old", 10).expect("old search").len(), 1);
        assert!(database.search("new", 10).expect("new search").is_empty());
    }

    #[test]
    fn watch_service_exposes_structured_index_diagnostics() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path().join("notes");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("invalid.md"), "---\ntitle: Missing delimiter\n")
            .expect("write invalid Markdown");
        let database_path = temporary_directory.path().join("index.sqlite3");
        let config = ScanConfig::default_for_home(&root);
        let service = IndexWatchService::start(config, database_path).expect("start service");

        let deadline = Instant::now() + Duration::from_secs(2);
        while !service.status().initial_scan_complete && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let status = service.status();

        assert!(status.initial_scan_complete);
        assert_eq!(status.progress.processed_files, 1);
        assert_eq!(status.issue_count, 1);
        assert_eq!(status.recent_issues.len(), 1);
        assert!(status.recent_issues[0].path.ends_with("invalid.md"));
    }

    #[test]
    fn watch_service_recovers_a_corrupt_database() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path().join("notes");
        fs::create_dir(&root).expect("create root");
        let database_path = temporary_directory.path().join("index.sqlite3");
        fs::write(&database_path, "not SQLite").expect("write corrupt database");
        let config = ScanConfig::default_for_home(&root);

        let service = IndexWatchService::start(config, database_path).expect("start service");
        let status = service.status();

        assert!(status.database_recovery.is_some());
    }

    #[test]
    fn watch_service_retries_roots_that_become_available() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path().join("later");
        let database_path = temporary_directory.path().join("index.sqlite3");
        let config = ScanConfig::default_for_home(&root);
        let service =
            IndexWatchService::start(config, database_path.clone()).expect("start service");
        let failure_deadline = Instant::now() + Duration::from_secs(2);
        while service.status().watcher_restart_count == 0 && Instant::now() < failure_deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(service.status().watcher_restart_count >= 1);
        fs::create_dir(&root).expect("create delayed root");
        fs::write(root.join("available.md"), "available after retry").expect("write Markdown");

        let deadline = Instant::now() + Duration::from_secs(4);
        let mut indexed = false;
        while Instant::now() < deadline {
            indexed = IndexDatabase::open(&database_path)
                .and_then(|database| database.search("available", 10))
                .is_ok_and(|results| !results.is_empty());
            if service.status().watcher_connected && indexed {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }

        assert!(service.status().watcher_connected);
        assert!(service.status().watcher_restart_count >= 1);
        assert!(indexed);
    }

    #[cfg(unix)]
    #[test]
    fn incomplete_directory_scans_remove_known_malformed_content() {
        use std::os::unix::fs::symlink;
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path();
        let directory = root.join("notes");
        fs::create_dir(&directory).expect("notes");
        let invalid = directory.join("invalid.md");
        fs::write(&invalid, "previouscontent").expect("original note");
        fs::write(directory.join("valid.md"), "preservedcontent").expect("valid note");
        let mut config = ScanConfig::default_for_home(root);
        config.follow_symbolic_links = true;
        let mut database = IndexDatabase::open_in_memory().expect("database");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("initial index");
        fs::write(&invalid, "---\ntitle: missing delimiter\n").expect("malformed note");
        // A dangling link makes traversal incomplete regardless of user privileges.
        symlink(directory.join("missing"), directory.join("broken")).expect("dangling link");
        let report = IndexSynchronizer::new(&config, &mut database)
            .synchronize_paths(&[directory])
            .expect("reconcile directory");
        assert_eq!(report.issue_count, 2);
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
    fn shutdown_prevents_processing_a_directory_batch() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path();
        fs::write(root.join("note.md"), "body").expect("note");
        let config = ScanConfig::default_for_home(root);
        let mut database = IndexDatabase::open_in_memory().expect("database");
        let status = Mutex::new(WatchServiceStatus::default());
        process_watch_batch(
            &config,
            &mut database,
            &AtomicBool::new(true),
            &status,
            WatchBatch {
                paths: vec![root.to_path_buf()],
                needs_reconciliation: false,
            },
        );
        assert_eq!(database.document_count().expect("count"), 0);
        assert_eq!(status.into_inner().expect("status").revision, 0);
    }

    #[test]
    fn canceled_directory_scans_do_not_flush_or_prune_partial_results() {
        use std::cell::Cell;
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path();
        for name in ["one.md", "two.md", "three.md"] {
            fs::write(root.join(name), "cachedterm").expect("note");
        }
        let config = ScanConfig::default_for_home(root);
        let mut database = IndexDatabase::open_in_memory().expect("database");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("index");
        for name in ["one.md", "two.md", "three.md"] {
            fs::write(root.join(name), "updatedterm and a longer body").expect("update note");
        }
        let checks = Cell::new(0);
        let result = IndexSynchronizer::new(&config, &mut database).synchronize_paths_with_control(
            &[root.to_path_buf()],
            || {
                checks.set(checks.get() + 1);
                checks.get() >= 5
            },
        );
        assert!(matches!(result, Err(SynchronizationError::Cancelled)));
        assert_eq!(database.search("cachedterm", 10).expect("cached").len(), 3);
        assert!(
            database
                .search("updatedterm", 10)
                .expect("updated")
                .is_empty()
        );
    }

    #[test]
    fn unavailable_roots_do_not_interrupt_healthy_roots() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let healthy = temporary_directory.path().join("healthy");
        let delayed = temporary_directory.path().join("delayed");
        fs::create_dir(&healthy).expect("healthy root");
        let database_path = temporary_directory.path().join("index.sqlite3");
        let mut config = ScanConfig::default_for_home(&delayed);
        config.roots.push(healthy.clone());
        let service = IndexWatchService::start(config, database_path.clone()).expect("service");
        wait_for(|| service.status().initial_scan_complete);
        let note = healthy.join("note.md");
        fs::write(&note, "healthyterm").expect("healthy note");
        let search = |term: &str| {
            IndexDatabase::open(&database_path)
                .and_then(|database| database.search(term, 10))
                .is_ok_and(|results| !results.is_empty())
        };
        wait_for(|| search("healthyterm"));
        assert!(!service.status().watcher_connected);
        assert!(service.status().last_error.is_some());

        fs::create_dir(&delayed).expect("delayed root");
        fs::write(delayed.join("later.md"), "delayedterm").expect("delayed note");
        wait_for(|| service.status().watcher_connected && search("delayedterm"));
        fs::write(&note, "updatedhealthyterm").expect("update healthy note");
        wait_for(|| search("updatedhealthyterm"));
        fs::remove_file(&note).expect("remove healthy note");
        wait_for(|| !search("updatedhealthyterm"));
    }

    #[test]
    fn explicit_sync_requests_index_new_notes_without_rebuilding() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        fs::create_dir(&root).unwrap();
        let config = ScanConfig::default_for_home(&root);
        let database_path = temp.path().join("index.sqlite3");
        let service = IndexWatchService::start(config.clone(), database_path.clone()).unwrap();
        wait_for(|| service.status().initial_scan_complete);
        let note =
            crate::notes::create_note(&config, &root.join("new/subfolder"), "createdterm.md")
                .unwrap();
        service.request_sync(note.path.clone());
        service.request_sync(note.path);
        wait_for(|| {
            IndexDatabase::open(&database_path)
                .and_then(|db| db.search("createdterm", 10))
                .is_ok_and(|results| results.len() == 1)
        });
        assert!(!service.rebuild_requested.load(Ordering::Acquire));
        assert!(!service.status().rebuild_in_progress);
    }

    fn wait_for(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if condition() {
                return;
            }
            assert!(Instant::now() < deadline, "condition did not become true");
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn pathless_rescan_notifications_reconcile_missed_changes() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path();
        let config = ScanConfig::default_for_home(root);
        let mut database = IndexDatabase::open_in_memory().expect("database");
        let removed = root.join("removed.md");
        let updated = root.join("updated.md");
        fs::write(&removed, "removedterm").expect("write removed note");
        fs::write(&updated, "oldterm").expect("write updated note");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("initial index");
        fs::remove_file(&removed).expect("remove note");
        fs::write(&updated, "newterm and a longer body").expect("update note");
        fs::write(root.join("created.md"), "createdterm").expect("create note");

        let mut watcher = FilesystemWatcher::new(&config).expect("watcher");
        let (sender, receiver) = mpsc::channel();
        watcher.receiver = receiver;
        watcher.debounce = Duration::ZERO;
        sender
            .send(Ok(
                Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan)
            ))
            .expect("send rescan");
        let batch = watcher.next_batch(Duration::ZERO).expect("rescan batch");
        assert!(batch.paths.is_empty());
        assert!(batch.needs_reconciliation);
        let status = Mutex::new(WatchServiceStatus::default());

        process_watch_batch(
            &config,
            &mut database,
            &AtomicBool::new(false),
            &status,
            batch,
        );

        assert!(
            database
                .search("removedterm", 10)
                .expect("removed search")
                .is_empty()
        );
        assert!(
            database
                .search("oldterm", 10)
                .expect("old search")
                .is_empty()
        );
        assert_eq!(
            database
                .search("newterm", 10)
                .expect("updated search")
                .len(),
            1
        );
        assert_eq!(
            database
                .search("createdterm", 10)
                .expect("created search")
                .len(),
            1
        );
        assert_eq!(status.into_inner().expect("status").revision, 1);
    }

    #[test]
    fn rescan_notifications_are_preserved_when_debounced_with_path_events() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let mut watcher = FilesystemWatcher::new(&config).expect("watcher");
        let (sender, receiver) = mpsc::channel();
        watcher.receiver = receiver;
        let path = temporary_directory.path().join("note.md");
        sender
            .send(Ok(Event::new(EventKind::Any).add_path(path.clone())))
            .expect("send path event");
        sender
            .send(Ok(
                Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan)
            ))
            .expect("send rescan");

        let batch = watcher.next_batch(Duration::ZERO).expect("mixed batch");

        assert_eq!(batch.paths, vec![path]);
        assert!(batch.needs_reconciliation);
    }

    #[test]
    fn native_watcher_reports_filesystem_changes() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let watcher = FilesystemWatcher::new(&config).expect("create watcher");
        fs::write(temporary_directory.path().join("created.md"), "body").expect("write Markdown");

        let paths = watcher
            .next_batch(Duration::from_secs(5))
            .expect("receive watcher event");

        assert!(!paths.paths.is_empty());
    }

    #[test]
    fn watcher_can_subscribe_to_a_configured_root() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let config = ScanConfig::default_for_home(temporary_directory.path());

        let watcher = FilesystemWatcher::new(&config).expect("create watcher");

        assert!(
            watcher
                .next_batch(Duration::ZERO)
                .expect("empty batch")
                .paths
                .is_empty()
        );
    }
}
