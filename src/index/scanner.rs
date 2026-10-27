use std::{collections::HashSet, fs, path::PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use thiserror::Error;
use walkdir::{DirEntry, WalkDir};

use crate::config::ScanConfig;

use super::document::modified_at_ns;

const MAX_RETAINED_ISSUES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    pub canonical_path: PathBuf,
    pub modified_at_ns: i64,
    pub file_size: u64,
}

#[derive(Debug, Default)]
pub struct ScanSummary {
    pub visited_files: usize,
    pub cancelled: bool,
    pub eligible_files: usize,
    pub issue_count: usize,
    pub issues: Vec<ScanIssue>,
    pub completed_roots: Vec<PathBuf>,
}

impl ScanSummary {
    fn record_issue(&mut self, issue: ScanIssue) {
        self.issue_count += 1;
        if self.issues.len() < MAX_RETAINED_ISSUES {
            self.issues.push(issue);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanIssue {
    pub path: PathBuf,
    pub kind: ScanIssueKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanIssueKind {
    Root,
    Walk,
    Metadata,
    Canonicalize,
    OutsideRoot,
    PathEncoding,
    FileTooLarge,
}

pub struct MarkdownScanner<'a> {
    config: &'a ScanConfig,
    excluded_globs: GlobSet,
    extensions: HashSet<&'a str>,
    excluded_directory_names: HashSet<&'a str>,
}

impl<'a> MarkdownScanner<'a> {
    pub fn new(config: &'a ScanConfig) -> Result<Self, ScannerError> {
        let mut glob_builder = GlobSetBuilder::new();
        for pattern in &config.excluded_globs {
            glob_builder.add(
                Glob::new(pattern).map_err(|source| ScannerError::InvalidGlob {
                    pattern: pattern.clone(),
                    source,
                })?,
            );
        }

        Ok(Self {
            config,
            excluded_globs: glob_builder.build()?,
            extensions: config.extensions.iter().map(String::as_str).collect(),
            excluded_directory_names: config
                .excluded_directory_names
                .iter()
                .map(String::as_str)
                .collect(),
        })
    }

    pub fn discover_file(
        &self,
        path: &std::path::Path,
    ) -> Result<Option<DiscoveredFile>, ScanIssue> {
        let symlink_metadata = fs::symlink_metadata(path).map_err(|error| ScanIssue {
            path: path.to_path_buf(),
            kind: ScanIssueKind::Metadata,
            message: error.to_string(),
        })?;
        if symlink_metadata.file_type().is_symlink() && !self.config.follow_symbolic_links {
            return Ok(None);
        }
        if !symlink_metadata.is_file() && !symlink_metadata.file_type().is_symlink() {
            return Ok(None);
        }

        let canonical_path = fs::canonicalize(path).map_err(|error| ScanIssue {
            path: path.to_path_buf(),
            kind: ScanIssueKind::Canonicalize,
            message: error.to_string(),
        })?;
        if canonical_path.to_str().is_none() {
            return Err(ScanIssue {
                path: canonical_path,
                kind: ScanIssueKind::PathEncoding,
                message: "path is not valid UTF-8".into(),
            });
        }

        let mut matching_root = None;
        for configured_root in &self.config.roots {
            if let Ok(root) = fs::canonicalize(configured_root)
                && canonical_path.starts_with(&root)
            {
                matching_root = Some(root);
                break;
            }
        }
        let Some(root) = matching_root else {
            return Ok(None);
        };
        let relative_path = canonical_path
            .strip_prefix(&root)
            .expect("a matching root is a canonical path prefix");
        if self.is_relative_path_excluded(relative_path, false) {
            return Ok(None);
        }
        if !has_supported_extension(&canonical_path, &self.extensions) {
            return Ok(None);
        }

        let metadata = fs::metadata(&canonical_path).map_err(|error| ScanIssue {
            path: canonical_path.clone(),
            kind: ScanIssueKind::Metadata,
            message: error.to_string(),
        })?;
        if metadata.len() > self.config.max_file_size_bytes {
            return Err(ScanIssue {
                path: canonical_path,
                kind: ScanIssueKind::FileTooLarge,
                message: format!(
                    "{} bytes exceeds the configured limit of {} bytes",
                    metadata.len(),
                    self.config.max_file_size_bytes
                ),
            });
        }

        Ok(Some(DiscoveredFile {
            canonical_path,
            modified_at_ns: modified_at_ns(&metadata),
            file_size: metadata.len(),
        }))
    }

    pub fn visit<E>(
        &self,
        visitor: impl FnMut(DiscoveredFile) -> Result<(), E>,
    ) -> Result<ScanSummary, E> {
        self.visit_with_control(visitor, || false)
    }

    pub fn visit_with_control<E>(
        &self,
        mut visitor: impl FnMut(DiscoveredFile) -> Result<(), E>,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> Result<ScanSummary, E> {
        let mut summary = ScanSummary::default();
        let mut seen_paths = HashSet::new();

        for configured_root in &self.config.roots {
            let root = match fs::canonicalize(configured_root) {
                Ok(root) => root,
                Err(error) => {
                    summary.record_issue(ScanIssue {
                        path: configured_root.clone(),
                        kind: ScanIssueKind::Root,
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            self.visit_walk_root(
                &root,
                &root,
                &mut seen_paths,
                &mut summary,
                &mut visitor,
                &mut is_cancelled,
            )?;
            if summary.cancelled {
                break;
            }
        }

        Ok(summary)
    }

    pub fn visit_subtree<E>(
        &self,
        directory: &std::path::Path,
        visitor: impl FnMut(DiscoveredFile) -> Result<(), E>,
    ) -> Result<ScanSummary, E> {
        self.visit_subtree_with_control(directory, visitor, || false)
    }

    pub fn visit_subtree_with_control<E>(
        &self,
        directory: &std::path::Path,
        mut visitor: impl FnMut(DiscoveredFile) -> Result<(), E>,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> Result<ScanSummary, E> {
        let mut summary = ScanSummary::default();
        if is_cancelled() {
            summary.cancelled = true;
            return Ok(summary);
        }
        let walk_root = match fs::canonicalize(directory) {
            Ok(path) => path,
            Err(error) => {
                summary.record_issue(ScanIssue {
                    path: directory.to_path_buf(),
                    kind: ScanIssueKind::Root,
                    message: error.to_string(),
                });
                return Ok(summary);
            }
        };

        let configured_root = self.config.roots.iter().find_map(|root| {
            let root = fs::canonicalize(root).ok()?;
            let relative_path = walk_root.strip_prefix(&root).ok()?;
            (!self.is_relative_path_excluded(relative_path, true)).then_some(root)
        });
        let Some(configured_root) = configured_root else {
            return Ok(summary);
        };

        self.visit_walk_root(
            &configured_root,
            &walk_root,
            &mut HashSet::new(),
            &mut summary,
            &mut visitor,
            &mut is_cancelled,
        )?;
        Ok(summary)
    }

    fn visit_walk_root<E>(
        &self,
        configured_root: &std::path::Path,
        walk_root: &std::path::Path,
        seen_paths: &mut HashSet<PathBuf>,
        summary: &mut ScanSummary,
        visitor: &mut impl FnMut(DiscoveredFile) -> Result<(), E>,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), E> {
        let mut root_complete = true;
        for entry in WalkDir::new(walk_root)
            .follow_links(self.config.follow_symbolic_links)
            .into_iter()
            .filter_entry(|entry| self.should_visit_entry(configured_root, entry))
        {
            if is_cancelled() {
                summary.cancelled = true;
                return Ok(());
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    root_complete = false;
                    summary.record_issue(ScanIssue {
                        path: error
                            .path()
                            .map_or_else(|| walk_root.to_path_buf(), std::path::Path::to_path_buf),
                        kind: ScanIssueKind::Walk,
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }

            summary.visited_files += 1;
            if !self.has_supported_extension(&entry) {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    root_complete = false;
                    summary.record_issue(ScanIssue {
                        path: entry.path().to_path_buf(),
                        kind: ScanIssueKind::Metadata,
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            if metadata.len() > self.config.max_file_size_bytes {
                summary.record_issue(ScanIssue {
                    path: entry.path().to_path_buf(),
                    kind: ScanIssueKind::FileTooLarge,
                    message: format!(
                        "{} bytes exceeds the configured limit of {} bytes",
                        metadata.len(),
                        self.config.max_file_size_bytes
                    ),
                });
                continue;
            }

            let canonical_path = match fs::canonicalize(entry.path()) {
                Ok(path) => path,
                Err(error) => {
                    root_complete = false;
                    summary.record_issue(ScanIssue {
                        path: entry.path().to_path_buf(),
                        kind: ScanIssueKind::Canonicalize,
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            if !canonical_path.starts_with(configured_root) {
                root_complete = false;
                summary.record_issue(ScanIssue {
                    path: canonical_path,
                    kind: ScanIssueKind::OutsideRoot,
                    message: "canonical path is outside the configured scan root".into(),
                });
                continue;
            }
            if canonical_path.to_str().is_none() {
                root_complete = false;
                summary.record_issue(ScanIssue {
                    path: canonical_path,
                    kind: ScanIssueKind::PathEncoding,
                    message: "path is not valid UTF-8".into(),
                });
                continue;
            }
            if !seen_paths.insert(canonical_path.clone()) {
                continue;
            }

            summary.eligible_files += 1;
            visitor(DiscoveredFile {
                canonical_path,
                modified_at_ns: modified_at_ns(&metadata),
                file_size: metadata.len(),
            })?;
        }

        if root_complete && !summary.completed_roots.contains(&walk_root.to_path_buf()) {
            summary.completed_roots.push(walk_root.to_path_buf());
        }
        Ok(())
    }

    fn should_visit_entry(&self, root: &std::path::Path, entry: &DirEntry) -> bool {
        if entry.depth() == 0 {
            return true;
        }

        if entry.file_type().is_dir()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| self.excluded_directory_names.contains(name))
        {
            return false;
        }

        let Ok(relative_path) = entry.path().strip_prefix(root) else {
            return false;
        };
        !self.is_relative_path_excluded(relative_path, entry.file_type().is_dir())
    }

    fn is_relative_path_excluded(
        &self,
        relative_path: &std::path::Path,
        is_directory: bool,
    ) -> bool {
        let directory_path = if is_directory {
            relative_path
        } else {
            relative_path.parent().unwrap_or(relative_path)
        };
        if directory_path.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| self.excluded_directory_names.contains(name))
        }) {
            return true;
        }

        let normalized_path = relative_path
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        self.excluded_globs.is_match(normalized_path)
    }

    pub(crate) fn allows_new_file(&self, relative_path: &std::path::Path) -> bool {
        has_supported_extension(relative_path, &self.extensions)
            && !self.is_relative_path_excluded(relative_path, false)
            && !relative_path.parent().is_some_and(|parent| {
                parent.ancestors().any(|ancestor| {
                    !ancestor.as_os_str().is_empty()
                        && self.is_relative_path_excluded(ancestor, true)
                })
            })
    }

    fn has_supported_extension(&self, entry: &DirEntry) -> bool {
        has_supported_extension(entry.path(), &self.extensions)
    }
}

fn has_supported_extension(path: &std::path::Path, extensions: &HashSet<&str>) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .is_some_and(|extension| extensions.contains(extension.as_str()))
}

#[derive(Debug, Error)]
pub enum ScannerError {
    #[error("invalid exclusion glob `{pattern}`: {source}")]
    InvalidGlob {
        pattern: String,
        #[source]
        source: globset::Error,
    },
    #[error("failed to build exclusion globs: {0}")]
    GlobSet(#[from] globset::Error),
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn recursively_discovers_supported_files_and_applies_exclusions() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path();
        fs::create_dir_all(root.join("notes/nested")).expect("create notes");
        fs::create_dir_all(root.join("node_modules/package")).expect("create excluded directory");
        fs::create_dir_all(root.join("Library/Caches/app")).expect("create cache directory");
        fs::write(root.join("notes/one.md"), "one").expect("write Markdown");
        fs::write(root.join("notes/nested/two.MARKDOWN"), "two").expect("write Markdown");
        fs::write(root.join("notes/ignored.txt"), "ignored").expect("write text");
        fs::write(root.join("node_modules/package/readme.md"), "excluded")
            .expect("write excluded Markdown");
        fs::write(root.join("Library/Caches/app/cache.md"), "excluded")
            .expect("write cached Markdown");

        let config = ScanConfig::default_for_home(root);
        let scanner = MarkdownScanner::new(&config).expect("scanner");
        let mut discovered = Vec::new();
        let summary = scanner
            .visit(|file| {
                discovered.push(file.canonical_path);
                Ok::<_, Infallible>(())
            })
            .expect("scan");

        discovered.sort();
        assert_eq!(discovered.len(), 2);
        assert!(discovered.iter().any(|path| path.ends_with("one.md")));
        assert!(discovered.iter().any(|path| path.ends_with("two.MARKDOWN")));
        assert_eq!(summary.eligible_files, 2);
        assert_eq!(summary.issue_count, 0);
    }

    #[test]
    fn scans_only_an_eligible_subtree_with_root_relative_exclusions() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path();
        fs::create_dir_all(root.join("notes/selected")).expect("create selected directory");
        fs::create_dir_all(root.join("notes/other")).expect("create other directory");
        fs::create_dir_all(root.join("Library/Caches/app")).expect("create excluded directory");
        fs::write(root.join("notes/selected/one.md"), "one").expect("write Markdown");
        fs::write(root.join("notes/other/two.md"), "two").expect("write Markdown");
        fs::write(root.join("Library/Caches/app/cache.md"), "cache").expect("write Markdown");
        let config = ScanConfig::default_for_home(root);
        let scanner = MarkdownScanner::new(&config).expect("scanner");
        let mut discovered = Vec::new();

        let summary = scanner
            .visit_subtree(&root.join("notes/selected"), |file| {
                discovered.push(file.canonical_path);
                Ok::<_, Infallible>(())
            })
            .expect("scan subtree");
        let excluded = scanner
            .visit_subtree(&root.join("Library/Caches"), |_| Ok::<_, Infallible>(()))
            .expect("skip excluded subtree");

        assert_eq!(discovered.len(), 1);
        assert!(discovered[0].ends_with("notes/selected/one.md"));
        assert_eq!(summary.completed_roots.len(), 1);
        assert_eq!(excluded.eligible_files, 0);
        assert!(excluded.completed_roots.is_empty());
    }

    #[test]
    fn subtree_scans_stop_without_marking_canceled_roots_complete() {
        use std::cell::Cell;
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let root = temporary_directory.path();
        fs::write(root.join("one.md"), "one").expect("note");
        fs::write(root.join("two.md"), "two").expect("note");
        let config = ScanConfig::default_for_home(root);
        let scanner = MarkdownScanner::new(&config).expect("scanner");
        let canceled = Cell::new(false);
        let summary = scanner
            .visit_subtree_with_control(
                root,
                |_| {
                    canceled.set(true);
                    Ok::<_, Infallible>(())
                },
                || canceled.get(),
            )
            .expect("scan");
        assert!(summary.cancelled);
        assert_eq!(summary.eligible_files, 1);
        assert!(summary.completed_roots.is_empty());
    }

    #[test]
    fn reports_oversized_files_without_visiting_them() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("large.md");
        fs::write(&path, "too large").expect("write Markdown");
        let mut config = ScanConfig::default_for_home(temporary_directory.path());
        config.max_file_size_bytes = 2;
        let scanner = MarkdownScanner::new(&config).expect("scanner");

        let summary = scanner.visit(|_| Ok::<_, Infallible>(())).expect("scan");

        assert_eq!(summary.eligible_files, 0);
        assert_eq!(summary.issue_count, 1);
        assert_eq!(summary.issues[0].kind, ScanIssueKind::FileTooLarge);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symbolic_links_by_default() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let outside_directory = tempfile::tempdir().expect("outside directory");
        fs::write(outside_directory.path().join("outside.md"), "outside").expect("write Markdown");
        symlink(
            outside_directory.path(),
            temporary_directory.path().join("linked"),
        )
        .expect("create symlink");
        let config = ScanConfig::default_for_home(temporary_directory.path());
        let scanner = MarkdownScanner::new(&config).expect("scanner");

        let summary = scanner.visit(|_| Ok::<_, Infallible>(())).expect("scan");

        assert_eq!(summary.eligible_files, 0);
    }
}
