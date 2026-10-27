use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use directories::UserDirs;

use crate::{config::ScanConfig, index::MarkdownScanner};

#[derive(Debug)]
pub struct CreatedNote {
    pub path: PathBuf,
    pub title: String,
    pub body: String,
    pub warning: Option<String>,
}

struct Destination {
    root: PathBuf,
    directory: PathBuf,
}

pub fn suggested_filename(query: &str, config: &ScanConfig) -> String {
    let title = query
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':') {
                '-'
            } else {
                ch
            }
        })
        .scan(0, |bytes, ch| {
            *bytes += ch.len_utf8();
            (*bytes <= 180).then_some(ch)
        })
        .collect::<String>();
    let title = title.trim().trim_matches('.').trim();
    let title = if title.is_empty() { "Untitled" } else { title };
    let extension = config
        .extensions
        .iter()
        .find(|extension| extension.as_str() == "md")
        .or_else(|| config.extensions.first())
        .map_or("md", String::as_str);
    if Path::new(title)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            config
                .extensions
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(extension))
        })
    {
        title.to_string()
    } else {
        format!("{title}.{extension}")
    }
}

pub fn suggested_destination(config: &ScanConfig, remembered: Option<&Path>) -> Option<PathBuf> {
    let filename = suggested_filename("Untitled", config);
    remembered
        .map(Path::to_path_buf)
        .into_iter()
        .chain(UserDirs::new().and_then(|dirs| {
            dirs.document_dir()
                .map(|documents| documents.join("Markdown Launcher Notes"))
        }))
        .chain(config.roots.iter().cloned())
        .find(|directory| validate_destination(config, directory, &filename).is_ok())
}

fn validate_filename(config: &ScanConfig, filename: &str) -> Result<(), String> {
    if filename.is_empty()
        || filename.trim() != filename
        || matches!(filename, "." | "..")
        || filename
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\' | ':'))
        || filename.ends_with('.')
    {
        return Err("Enter a filename without path separators, control characters, leading/trailing spaces, or a trailing dot".into());
    }
    if !Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            config
                .extensions
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(extension))
        })
    {
        return Err(format!(
            "Use a configured file extension: {}",
            config.extensions.join(", ")
        ));
    }
    Ok(())
}

fn validate_destination(
    config: &ScanConfig,
    directory: &Path,
    filename: &str,
) -> Result<Destination, String> {
    validate_filename(config, filename)?;
    if !directory.is_absolute()
        || directory
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("Choose an absolute destination without '..' path components".into());
    }
    let mut existing = directory;
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    existing
                        .file_name()
                        .ok_or("The destination is unavailable")?,
                );
                existing = existing.parent().ok_or("The destination is unavailable")?;
            }
            Err(error) => return Err(format!("Cannot inspect the destination: {error}")),
        }
    }
    let mut resolved = fs::canonicalize(existing)
        .map_err(|error| format!("The destination is unavailable: {error}"))?;
    if !resolved.is_dir() {
        return Err("The destination must be a folder".into());
    }
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    if resolved.to_str().is_none() {
        return Err("The destination must be a valid UTF-8 path".into());
    }
    let scanner = MarkdownScanner::new(config).map_err(|error| error.to_string())?;
    for configured_root in &config.roots {
        let Ok(root) = fs::canonicalize(configured_root) else {
            continue;
        };
        let Ok(relative) = resolved.strip_prefix(&root) else {
            continue;
        };
        if !config.follow_symbolic_links
            && directory.ancestors().any(|ancestor| {
                !configured_root.starts_with(ancestor)
                    && fs::symlink_metadata(ancestor)
                        .is_ok_and(|metadata| metadata.file_type().is_symlink())
            })
        {
            return Err("Symbolic-link destinations are disabled in Settings".into());
        }
        if !scanner.allows_new_file(&relative.join(filename)) {
            return Err(
                "This destination or filename is excluded by the active scan settings".into(),
            );
        }
        return Ok(Destination {
            root,
            directory: resolved,
        });
    }
    Err("Choose a destination inside one of the active scan roots in Settings".into())
}

#[cfg(target_os = "macos")]
pub fn check_destination(
    config: &ScanConfig,
    directory: &Path,
    filename: &str,
) -> Result<(), String> {
    validate_destination(config, directory, filename).map(|_| ())
}

pub fn create_note(
    config: &ScanConfig,
    directory: &Path,
    filename: &str,
) -> Result<CreatedNote, String> {
    let destination = validate_destination(config, directory, filename)?;
    let title = Path::new(filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .ok_or("Enter a non-empty note name")?
        .to_string();
    let body = format!("# {title}\n");
    if body.len() as u64 > config.max_file_size_bytes {
        return Err("The initial note exceeds the configured maximum file size".into());
    }
    match fs::symlink_metadata(destination.directory.join(filename)) {
        Ok(_) => {
            return Err(
                "A file or folder with this name already exists. Choose another filename".into(),
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Cannot inspect this filename: {error}")),
    }
    let warning = publish_note(&destination, filename, body.as_bytes())?;
    Ok(CreatedNote {
        path: destination.directory.join(filename),
        title,
        body,
        warning,
    })
}

#[cfg(unix)]
fn publish_note(
    destination: &Destination,
    filename: &str,
    body: &[u8],
) -> Result<Option<String>, String> {
    use rustix::fs::{AtFlags, Mode, OFlags, fsync, linkat, mkdirat, open, openat, unlinkat};
    use std::{
        io::Write as _,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    // Resolve the canonical root through directory handles. No component may
    // become a symlink between validation and writing; children stay anchored
    // even if an ancestor is renamed while creation is in flight.
    let mut directory = open("/", flags, Mode::empty()).map_err(|error| error.to_string())?;
    for part in destination.root.components() {
        if let Component::Normal(name) = part {
            directory = openat(&directory, name, flags, Mode::empty())
                .map_err(|error| format!("The scan root changed or is inaccessible: {error}"))?;
        }
    }
    let relative = destination
        .directory
        .strip_prefix(&destination.root)
        .map_err(|error| error.to_string())?;
    for part in relative.components() {
        let Component::Normal(name) = part else {
            return Err("Invalid destination component".into());
        };
        directory = match openat(&directory, name, flags, Mode::empty()) {
            Ok(child) => child,
            Err(rustix::io::Errno::NOENT) => {
                match mkdirat(&directory, name, Mode::from_raw_mode(0o755)) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => {
                        return Err(format!("Could not create the destination folder: {error}"));
                    }
                }
                fsync(&directory).map_err(|error| {
                    format!("Could not synchronize the destination folder: {error}")
                })?;
                openat(&directory, name, flags, Mode::empty()).map_err(|error| {
                    format!("The destination changed or is inaccessible: {error}")
                })?
            }
            Err(error) => {
                return Err(format!(
                    "The destination changed or is inaccessible: {error}"
                ));
            }
        };
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = format!(
        ".markdown-launcher-{}-{nonce}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    );
    let fd = openat(
        &directory,
        &temporary,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o644),
    )
    .map_err(|error| format!("Cannot write to the destination: {error}"))?;
    let result = (|| {
        let mut file = fs::File::from(fd);
        file.write_all(body)
            .map_err(|error| format!("Could not write the note: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Could not synchronize the note: {error}"))?;
        // A hard link publishes a complete file atomically and cannot replace
        // any existing file, directory, or symlink, including a racing creator.
        linkat(
            &directory,
            &temporary,
            &directory,
            filename,
            AtFlags::empty(),
        )
        .map_err(|error| {
            if error == rustix::io::Errno::EXIST {
                "A file or folder with this name already exists. Choose another filename".into()
            } else {
                format!("Could not create the note: {error}")
            }
        })
    })();
    let cleanup = unlinkat(&directory, &temporary, AtFlags::empty());
    result?;
    let sync = fsync(&directory);
    Ok(cleanup
        .err()
        .map(|error| format!("Note created, but temporary-file cleanup failed: {error}"))
        .or_else(|| {
            sync.err()
                .map(|error| format!("Note created, but folder synchronization failed: {error}"))
        }))
}

#[cfg(not(unix))]
fn publish_note(_: &Destination, _: &str, _: &[u8]) -> Result<Option<String>, String> {
    Err("Safe note creation is currently supported only on Unix platforms".into())
}

pub fn load_last_destination(path: &Path) -> Option<PathBuf> {
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > 16 * 1024 {
        return None;
    }
    use std::io::Read as _;
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 16 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

pub fn save_last_destination(path: &Path, directory: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("The note preference path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(directory).map_err(|error| error.to_string())?;
    use std::io::Write as _;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    file.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn creates_complete_notes_and_preserves_existing_files() {
        let temp = tempfile::tempdir().unwrap();
        let config = ScanConfig::default_for_home(temp.path());
        let destination = temp.path().join("new/subfolder");
        let note = create_note(&config, &destination, "Café.md").unwrap();
        assert_eq!(fs::read_to_string(&note.path).unwrap(), "# Café\n");
        assert!(note.warning.is_none());
        assert!(
            create_note(&config, &destination, "Café.md")
                .unwrap_err()
                .contains("already exists")
        );
        assert_eq!(fs::read_to_string(&note.path).unwrap(), "# Café\n");
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
        assert!(
            MarkdownScanner::new(&config)
                .unwrap()
                .discover_file(&note.path)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn rejects_invalid_names_outside_roots_and_excluded_paths_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        let mut config = ScanConfig::default_for_home(&root);
        config.excluded_globs = vec!["private".into(), "**/secret*".into()];
        for name in [
            "",
            "../oops.md",
            "sub/note.md",
            "sub\\note.md",
            "a:b.md",
            "x\n.md",
            "x.txt",
            ".md",
            "note.md ",
            "note",
            "..",
        ] {
            assert!(create_note(&config, &root, name).is_err(), "{name}");
        }
        for folder in [
            temp.path().join("root-other"),
            root.join("node_modules/nested"),
            root.join("private/nested"),
        ] {
            assert!(
                create_note(&config, &folder, "note.md").is_err(),
                "{}",
                folder.display()
            );
            assert!(!folder.exists());
        }
        assert!(create_note(&config, &root, "secret.md").is_err());
        assert_eq!(fs::read_dir(root).unwrap().count(), 0);
    }

    #[test]
    fn symlinks_cannot_escape_roots_and_obey_the_scan_policy() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let inside = root.join("inside");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&inside).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&inside, root.join("alias")).unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        symlink(outside.join("missing.md"), root.join("dangling.md")).unwrap();
        let mut config = ScanConfig::default_for_home(&root);
        assert!(create_note(&config, &root.join("alias"), "note.md").is_err());
        assert!(
            create_note(&config, &root, "dangling.md")
                .unwrap_err()
                .contains("already exists")
        );
        config.follow_symbolic_links = true;
        assert!(create_note(&config, &root.join("escape"), "note.md").is_err());
        assert!(create_note(&config, &root.join("alias"), "note.md").is_ok());
        assert!(!outside.join("note.md").exists());
    }

    #[test]
    fn secure_publication_rejects_a_directory_symlink_swap() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let folder = root.join("folder");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&folder).unwrap();
        fs::create_dir(&outside).unwrap();
        let config = ScanConfig::default_for_home(&root);
        let destination = validate_destination(&config, &folder, "note.md").unwrap();
        fs::remove_dir(&folder).unwrap();
        symlink(&outside, &folder).unwrap();
        assert!(publish_note(&destination, "note.md", b"complete").is_err());
        assert!(!outside.join("note.md").exists());
    }

    #[test]
    fn simultaneous_creators_never_overwrite_each_other() {
        let temp = tempfile::tempdir().unwrap();
        let config = ScanConfig::default_for_home(temp.path());
        let destination = validate_destination(&config, temp.path(), "note.md").unwrap();
        let barrier = std::sync::Barrier::new(8);
        let results = std::thread::scope(|scope| {
            let handles = (0..8)
                .map(|index| {
                    let destination = &destination;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        publish_note(destination, "note.md", format!("writer {index}").as_bytes())
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(
            fs::read_to_string(temp.path().join("note.md"))
                .unwrap()
                .starts_with("writer ")
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn preferences_and_suggestions_do_not_change_scan_settings() {
        let temp = tempfile::tempdir().unwrap();
        let config = ScanConfig::default_for_home(temp.path());
        assert_eq!(suggested_filename("", &config), "Untitled.md");
        assert_eq!(suggested_filename("Hello.md", &config), "Hello.md");
        assert!(suggested_filename(&"🦀".repeat(200), &config).len() <= 183);
        assert_eq!(
            suggested_filename("Deploy/check\nlist", &config),
            "Deploy-check-list.md"
        );
        assert_eq!(
            suggested_destination(&config, Some(temp.path())),
            Some(temp.path().to_path_buf())
        );
        let preference = temp.path().join("state/note-creation.json");
        save_last_destination(&preference, temp.path()).unwrap();
        assert_eq!(
            load_last_destination(&preference),
            Some(temp.path().to_path_buf())
        );
        fs::write(&preference, "bad data").unwrap();
        assert_eq!(load_last_destination(&preference), None);
        assert_eq!(
            suggested_destination(&config, Some(Path::new("/not/an/allowed/root"))),
            Some(temp.path().to_path_buf())
        );
    }

    #[test]
    fn permission_errors_do_not_leave_partial_notes() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("readonly");
        fs::create_dir(&root).unwrap();
        let original = fs::metadata(&root).unwrap().permissions();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();
        // Privileged test runners may still write regardless of mode bits.
        let probe = fs::File::create(root.join("probe"));
        if probe.is_ok() {
            fs::set_permissions(&root, original).unwrap();
            return;
        }
        let config = ScanConfig::default_for_home(&root);
        let result = create_note(&config, &root, "note.md");
        fs::set_permissions(&root, original).unwrap();
        assert!(result.unwrap_err().contains("Cannot write"));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }

    #[test]
    fn any_configured_root_can_contain_new_notes() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        let mut config = ScanConfig::default_for_home(&first);
        config.roots.push(second.clone());
        let note = create_note(&config, &second.join("nested"), "note.markdown").unwrap();
        assert!(create_note(&config, &second, ".hidden-note.md").is_ok());
        assert!(note.path.starts_with(fs::canonicalize(&second).unwrap()));
        fs::create_dir(second.join("occupied.md")).unwrap();
        assert!(
            create_note(&config, &second, "occupied.md")
                .unwrap_err()
                .contains("already exists")
        );
    }

    #[test]
    fn root_symlink_swaps_are_rejected_after_validation() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        let config = ScanConfig::default_for_home(&root);
        let destination = validate_destination(&config, &root, "note.md").unwrap();
        fs::remove_dir(&root).unwrap();
        symlink(&outside, &root).unwrap();
        assert!(publish_note(&destination, "note.md", b"complete").is_err());
        assert!(!outside.join("note.md").exists());
    }

    #[test]
    fn size_limits_and_unavailable_roots_prevent_creation() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = ScanConfig::default_for_home(temp.path());
        config.max_file_size_bytes = 1;
        assert!(
            create_note(&config, temp.path(), "note.md")
                .unwrap_err()
                .contains("maximum")
        );
        config.roots = vec![temp.path().join("missing")];
        assert!(create_note(&config, temp.path(), "note.md").is_err());
        assert!(!temp.path().join("note.md").exists());
    }
}
