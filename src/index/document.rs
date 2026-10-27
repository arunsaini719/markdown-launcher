use std::{
    collections::HashSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::Deserialize;
use thiserror::Error;

use super::scanner::DiscoveredFile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub canonical_path: PathBuf,
    pub metadata_id: Option<String>,
    pub title: String,
    pub tags: Vec<String>,
    pub body: String,
    pub modified_at_ns: i64,
    pub file_size: u64,
    pub content_hash: String,
}

impl Document {
    pub fn parse(file: &DiscoveredFile, max_file_size_bytes: u64) -> Result<Self, ParseError> {
        let reader = fs::File::open(&file.canonical_path).map_err(|source| ParseError::Read {
            path: file.canonical_path.clone(),
            source,
        })?;
        let filesystem_metadata = reader
            .metadata()
            .map_err(|source| ParseError::ReadMetadata {
                path: file.canonical_path.clone(),
                source,
            })?;
        if filesystem_metadata.len() > max_file_size_bytes {
            return Err(ParseError::FileTooLarge {
                path: file.canonical_path.clone(),
                size: filesystem_metadata.len(),
                maximum: max_file_size_bytes,
            });
        }
        let bytes =
            read_bounded(reader, max_file_size_bytes).map_err(|source| ParseError::Read {
                path: file.canonical_path.clone(),
                source,
            })?;
        let actual_size = u64::try_from(bytes.len()).map_err(|_| ParseError::FileTooLarge {
            path: file.canonical_path.clone(),
            size: u64::MAX,
            maximum: max_file_size_bytes,
        })?;
        if actual_size > max_file_size_bytes {
            return Err(ParseError::FileTooLarge {
                path: file.canonical_path.clone(),
                size: actual_size,
                maximum: max_file_size_bytes,
            });
        }

        let content_hash = blake3::hash(&bytes).to_hex().to_string();
        let content = String::from_utf8(bytes).map_err(|source| ParseError::Encoding {
            path: file.canonical_path.clone(),
            source,
        })?;
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
        let (frontmatter, body) =
            split_frontmatter(content).map_err(|kind| ParseError::Frontmatter {
                path: file.canonical_path.clone(),
                kind,
            })?;
        let metadata = frontmatter
            .map(serde_saphyr::from_str::<Frontmatter>)
            .transpose()
            .map_err(|source| ParseError::Metadata {
                path: file.canonical_path.clone(),
                source: Box::new(source),
            })?
            .unwrap_or_default();

        let title = metadata
            .title
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
            .or_else(|| {
                file.canonical_path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_string)
            })
            .ok_or_else(|| ParseError::MissingTitle {
                path: file.canonical_path.clone(),
            })?;
        let tags = normalize_tags(metadata.tags.into_values());
        let metadata_id = metadata
            .id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty());
        Ok(Self {
            canonical_path: file.canonical_path.clone(),
            metadata_id,
            title,
            tags,
            body: body.to_string(),
            modified_at_ns: modified_at_ns(&filesystem_metadata),
            file_size: actual_size,
            content_hash,
        })
    }
}

fn read_bounded(reader: impl Read, maximum: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    id: Option<String>,
    title: Option<String>,
    #[serde(default)]
    tags: Tags,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum Tags {
    One(String),
    Many(Vec<String>),
    #[default]
    Missing,
}

impl Tags {
    fn into_values(self) -> Vec<String> {
        match self {
            Self::One(tag) => vec![tag],
            Self::Many(tags) => tags,
            Self::Missing => Vec::new(),
        }
    }
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::with_capacity(tags.len());
    tags.into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .filter(|tag| seen.insert(tag.clone()))
        .collect()
}

fn split_frontmatter(content: &str) -> Result<(Option<&str>, &str), FrontmatterError> {
    let Some(first_line_end) = content.find('\n') else {
        return if content.trim_end_matches('\r') == "---" {
            Err(FrontmatterError::MissingClosingDelimiter)
        } else {
            Ok((None, content))
        };
    };
    if content[..first_line_end].trim_end_matches('\r') != "---" {
        return Ok((None, content));
    }

    let metadata_start = first_line_end + 1;
    let mut line_start = metadata_start;
    for segment in content[metadata_start..].split_inclusive('\n') {
        let line = segment.trim_end_matches(['\r', '\n']);
        if line == "---" {
            let body_start = line_start + segment.len();
            return Ok((
                Some(&content[metadata_start..line_start]),
                &content[body_start..],
            ));
        }
        line_start += segment.len();
    }

    if content[line_start..].trim_end_matches('\r') == "---" {
        return Ok((Some(&content[metadata_start..line_start]), ""));
    }

    Err(FrontmatterError::MissingClosingDelimiter)
}

pub(crate) fn modified_at_ns(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_nanos()).ok())
        .unwrap_or(0)
}

pub fn read_markdown_body(path: &Path, max_file_size_bytes: u64) -> Result<String, ParseError> {
    let metadata = fs::metadata(path).map_err(|source| ParseError::ReadMetadata {
        path: path.to_path_buf(),
        source,
    })?;
    let discovered = DiscoveredFile {
        canonical_path: fs::canonicalize(path).map_err(|source| ParseError::ReadMetadata {
            path: path.to_path_buf(),
            source,
        })?,
        modified_at_ns: modified_at_ns(&metadata),
        file_size: metadata.len(),
    };

    Document::parse(&discovered, max_file_size_bytes).map(|document| document.body)
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read metadata for {path}: {source}")]
    ReadMetadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid UTF-8: {source}")]
    Encoding {
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("invalid frontmatter in {path}: {kind}")]
    Frontmatter {
        path: PathBuf,
        kind: FrontmatterError,
    },
    #[error("failed to parse YAML metadata in {path}: {source}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: Box<serde_saphyr::Error>,
    },
    #[error("cannot determine a title for {path}")]
    MissingTitle { path: PathBuf },
    #[error("{path} is {size} bytes, above the configured limit of {maximum} bytes")]
    FileTooLarge {
        path: PathBuf,
        size: u64,
        maximum: u64,
    },
}

impl ParseError {
    pub(crate) fn invalidates_cached_content(&self) -> bool {
        match self {
            Self::Read { source, .. } | Self::ReadMetadata { source, .. } => {
                source.kind() == io::ErrorKind::NotFound
            }
            Self::Encoding { .. }
            | Self::Frontmatter { .. }
            | Self::Metadata { .. }
            | Self::MissingTitle { .. }
            | Self::FileTooLarge { .. } => true,
        }
    }
}

#[derive(Debug, Error)]
pub enum FrontmatterError {
    #[error("the closing `---` delimiter is missing")]
    MissingClosingDelimiter,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_preserves_the_markdown_body() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("tokio.md");
        fs::write(
            &path,
            "---\nid: snippet-1\ntitle: Tokio main\ntags: [rust, async, rust]\n---\n#[tokio::main]\nasync fn main() {}\n",
        )
        .expect("write Markdown");

        let document = parse_path(&path).expect("parse document");

        assert_eq!(document.metadata_id.as_deref(), Some("snippet-1"));
        assert_eq!(document.title, "Tokio main");
        assert_eq!(document.tags, ["rust", "async"]);
        assert_eq!(document.body, "#[tokio::main]\nasync fn main() {}\n");
        assert!(!document.content_hash.is_empty());
    }

    #[test]
    fn accepts_a_single_tag_and_uses_the_filename_as_the_title() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("docker-cleanup.md");
        fs::write(&path, "---\ntags: docker\n---\ndocker system prune\n").expect("write Markdown");

        let document = parse_path(&path).expect("parse document");

        assert_eq!(document.title, "docker-cleanup");
        assert_eq!(document.tags, ["docker"]);
        assert_eq!(document.body, "docker system prune\n");
    }

    #[test]
    fn treats_a_plain_markdown_file_as_the_body() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("plain.markdown");
        fs::write(&path, "# Plain note\n").expect("write Markdown");

        let document = parse_path(&path).expect("parse document");

        assert_eq!(document.title, "plain");
        assert!(document.tags.is_empty());
        assert_eq!(document.body, "# Plain note\n");
    }

    #[test]
    fn clipboard_body_reads_the_latest_file_contents() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("current.md");
        fs::write(&path, "---\ntitle: Initial\n---\nold body\n").expect("write Markdown");
        let indexed = parse_path(&path).expect("parse indexed document");
        fs::write(&path, "---\ntitle: Updated\n---\nlatest body\n").expect("update Markdown");

        let body = read_markdown_body(&path, 1024 * 1024).expect("read current body");

        assert_eq!(indexed.body, "old body\n");
        assert_eq!(body, "latest body\n");
    }

    #[test]
    fn rejects_unclosed_frontmatter() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("invalid.md");
        fs::write(&path, "---\ntitle: Invalid\n").expect("write Markdown");

        let error = parse_path(&path).expect_err("frontmatter should be invalid");

        assert!(matches!(error, ParseError::Frontmatter { .. }));
    }

    #[test]
    fn unreadable_files_are_not_treated_as_invalid_content() {
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::Interrupted,
            io::ErrorKind::Other,
        ] {
            let error = ParseError::Read {
                path: "note.md".into(),
                source: io::Error::from(kind),
            };
            assert!(!error.invalidates_cached_content());
            let error = ParseError::ReadMetadata {
                path: "note.md".into(),
                source: io::Error::from(kind),
            };
            assert!(!error.invalidates_cached_content());
        }
        let missing = ParseError::Read {
            path: "note.md".into(),
            source: io::ErrorKind::NotFound.into(),
        };
        assert!(missing.invalidates_cached_content());
        let malformed = ParseError::Frontmatter {
            path: "note.md".into(),
            kind: FrontmatterError::MissingClosingDelimiter,
        };
        assert!(malformed.invalidates_cached_content());
    }

    #[test]
    fn bounded_reads_stop_after_the_overflow_probe() {
        let mut reader = io::Cursor::new(vec![b'a'; 4096]);
        let bytes = read_bounded(&mut reader, 4).expect("bounded read");
        assert_eq!(bytes.len(), 5);
        assert_eq!(reader.position(), 5);
    }

    #[test]
    fn clipboard_reads_accept_the_limit_and_reject_oversized_files() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("note.md");
        fs::write(&path, "12345").expect("write note");
        assert_eq!(read_markdown_body(&path, 5).expect("exact limit"), "12345");
        let file = fs::File::create(&path).expect("create large file");
        file.set_len(1024 * 1024 * 1024)
            .expect("sparse oversized file");
        assert!(matches!(
            read_markdown_body(&path, 5),
            Err(ParseError::FileTooLarge { .. })
        ));
    }

    #[test]
    fn parsing_rechecks_size_after_discovery() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let path = temporary_directory.path().join("note.md");
        fs::write(&path, "small").expect("small note");
        let discovered = DiscoveredFile {
            canonical_path: path.canonicalize().expect("canonical path"),
            modified_at_ns: 0,
            file_size: 5,
        };
        fs::write(&path, "now larger than the limit").expect("grow note");
        assert!(matches!(
            Document::parse(&discovered, 5),
            Err(ParseError::FileTooLarge { .. })
        ));
    }

    fn parse_path(path: &Path) -> Result<Document, ParseError> {
        let canonical_path = fs::canonicalize(path).expect("canonical path");
        let metadata = fs::metadata(&canonical_path).expect("metadata");
        Document::parse(
            &DiscoveredFile {
                canonical_path,
                modified_at_ns: modified_at_ns(&metadata),
                file_size: metadata.len(),
            },
            1024 * 1024,
        )
    }
}
