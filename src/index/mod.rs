mod database;
mod document;
mod indexer;
mod scanner;
mod synchronizer;

pub use database::{DatabaseError, DatabaseRecovery, IndexDatabase, SearchResult};
pub use document::{Document, FrontmatterError, ParseError, read_markdown_body};
pub use indexer::{
    IndexingError, IndexingIssue, IndexingProgress, IndexingReport, MarkdownIndexer,
};
pub use scanner::{
    DiscoveredFile, MarkdownScanner, ScanIssue, ScanIssueKind, ScanSummary, ScannerError,
};
pub use synchronizer::{
    FilesystemWatcher, IndexActivity, IndexDiagnostic, IndexSynchronizer, IndexWatchService,
    SynchronizationError, SynchronizationIssue, SynchronizationReport, WatchBatch, WatchError,
    WatchServiceError, WatchServiceStatus,
};
