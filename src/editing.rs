use std::{fs, io::Read as _, path::Path, time::SystemTime};

use crate::{config::ScanConfig, index::MarkdownScanner};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileVersion {
    hash: blake3::Hash,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl FileVersion {
    pub(crate) fn matches_text(&self, text: &str) -> bool {
        self.hash == blake3::hash(text.as_bytes())
    }

    fn of(file: &fs::File, bytes: &[u8]) -> Result<Self, String> {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;
        Ok(Self {
            hash: blake3::hash(bytes),
            modified: metadata.modified().map_err(|error| error.to_string())?,
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        })
    }
}

#[derive(Debug)]
pub struct EditSource {
    pub text: String,
    pub version: FileVersion,
}

#[derive(Debug)]
pub struct SaveOutcome {
    pub version: FileVersion,
    pub warning: Option<String>,
}

pub fn load(config: &ScanConfig, path: &Path) -> Result<EditSource, String> {
    let (_, file) = open_source(config, path, false)?;
    let (bytes, version) = read_source(file, config.max_file_size_bytes)?;
    let text =
        String::from_utf8(bytes).map_err(|error| format!("Not a UTF-8 document: {error}"))?;
    Ok(EditSource { text, version })
}

fn read_source(mut file: fs::File, maximum: u64) -> Result<(Vec<u8>, FileVersion), String> {
    let before = FileVersion::of(&file, &[])?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read the source: {error}"))?;
    if bytes.len() as u64 > maximum {
        return Err(format!(
            "File exceeds the configured {maximum}-byte size limit"
        ));
    }
    if before != FileVersion::of(&file, &[])? {
        return Err("Conflict: the file changed while being read".into());
    }
    let version = FileVersion::of(&file, &bytes)?;
    Ok((bytes, version))
}

#[cfg(unix)]
fn open_source(
    config: &ScanConfig,
    path: &Path,
    writable: bool,
) -> Result<(rustix::fd::OwnedFd, fs::File), String> {
    use rustix::fs::{Mode, OFlags, open, openat};
    use std::path::Component;
    if !path.is_absolute() {
        return Err("The document path must be absolute".into());
    }
    let canonical =
        fs::canonicalize(path).map_err(|error| format!("The source is unavailable: {error}"))?;
    if canonical != path {
        return Err(
            "The source path changed or became a symbolic link; reopen it before editing".into(),
        );
    }
    let scanner = MarkdownScanner::new(config).map_err(|error| error.to_string())?;
    config
        .roots
        .iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .find(|root| {
            canonical
                .strip_prefix(root)
                .is_ok_and(|relative| scanner.allows_new_file(relative))
        })
        .ok_or("The document is outside the active roots or excluded by scan settings")?;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open("/", flags, Mode::empty()).map_err(|error| error.to_string())?;
    for component in canonical
        .parent()
        .ok_or("The document has no parent")?
        .components()
    {
        if let Component::Normal(name) = component {
            directory = openat(&directory, name, flags, Mode::empty()).map_err(|error| {
                format!("The source directory changed or is inaccessible: {error}")
            })?;
        }
    }
    let file_flags = if writable {
        OFlags::RDWR
    } else {
        OFlags::RDONLY
    };
    let file = fs::File::from(
        openat(
            &directory,
            canonical.file_name().ok_or("Missing filename")?,
            file_flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            format!(
                "Could not {} the source: {error}",
                if writable { "write" } else { "read" }
            )
        })?,
    );
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("The source is not a regular file".into());
    }
    Ok((directory, file))
}

#[cfg(not(unix))]
fn open_source(_: &ScanConfig, _: &Path, _: bool) -> Result<((), fs::File), String> {
    Err("Safe editing is currently supported only on Unix platforms".into())
}

#[cfg(unix)]
pub fn save(
    config: &ScanConfig,
    path: &Path,
    expected: &FileVersion,
    text: &str,
) -> Result<SaveOutcome, String> {
    save_with_hook(config, path, expected, text, || {})
}

#[cfg(unix)]
fn save_with_hook(
    config: &ScanConfig,
    path: &Path,
    expected: &FileVersion,
    text: &str,
    before_recheck: impl FnOnce(),
) -> Result<SaveOutcome, String> {
    use rustix::fs::{AtFlags, Mode, OFlags, fstat, fsync, openat, renameat, unlinkat};
    use std::{
        io::Write as _,
        os::unix::fs::MetadataExt as _,
        sync::atomic::{AtomicU64, Ordering},
        time::UNIX_EPOCH,
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if text.len() as u64 > config.max_file_size_bytes {
        return Err(format!(
            "Draft exceeds the configured {}-byte size limit",
            config.max_file_size_bytes
        ));
    }
    let (directory, source) = open_source(config, path, true)?;
    let (_, current) = read_source(
        source.try_clone().map_err(|error| error.to_string())?,
        config.max_file_size_bytes,
    )?;
    if current != *expected {
        return Err(
            "Conflict: this file changed outside the editor. Reopen it to use the disk version"
                .into(),
        );
    }
    if expected.matches_text(text) {
        return Ok(SaveOutcome {
            version: current,
            warning: None,
        });
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = format!(
        ".markdown-edit-{}-{nonce}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let mut file = fs::File::from(
        openat(
            &directory,
            &temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|error| format!("Could not create a temporary save file: {error}"))?,
    );
    let result = (|| {
        #[cfg(target_os = "macos")]
        macos_file_metadata::copy_metadata(&source, &file)
            .map_err(|error| format!("Could not preserve file metadata: {error}"))?;
        #[cfg(not(target_os = "macos"))]
        file.set_permissions(
            source
                .metadata()
                .map_err(|error| error.to_string())?
                .permissions(),
        )
        .map_err(|error| error.to_string())?;
        let original = source.metadata().map_err(|error| error.to_string())?;
        let copied = file.metadata().map_err(|error| error.to_string())?;
        if (original.uid(), original.gid()) != (copied.uid(), copied.gid()) {
            return Err(
                "Cannot preserve the source file's ownership; source left unchanged".into(),
            );
        }
        file.write_all(text.as_bytes())
            .map_err(|error| format!("Could not write the draft: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Could not synchronize the draft: {error}"))?;
        before_recheck();
        // Reopen the name just before replacement: checking the old descriptor
        // alone would miss another editor atomically replacing that name.
        let (latest_directory, latest) = open_source(config, path, false)?;
        let original_directory = fstat(&directory).map_err(|error| error.to_string())?;
        let latest_directory = fstat(&latest_directory).map_err(|error| error.to_string())?;
        if (original_directory.st_dev, original_directory.st_ino)
            != (latest_directory.st_dev, latest_directory.st_ino)
        {
            return Err("Conflict: the source directory changed while saving".into());
        }
        let (_, current) = read_source(latest, config.max_file_size_bytes)?;
        if current != *expected {
            return Err("Conflict: this file changed while saving; source left unchanged".into());
        }
        let fallback = FileVersion::of(&file, text.as_bytes())?;
        renameat(
            &directory,
            &temporary,
            &directory,
            path.file_name().ok_or("Missing filename")?,
        )
        .map_err(|error| format!("Could not replace the source: {error}"))?;
        let (version, mut warning) = match FileVersion::of(&file, text.as_bytes()) {
            Ok(version) => (version, None),
            Err(error) => (
                fallback,
                Some(format!(
                    "Saved, but could not refresh file metadata: {error}"
                )),
            ),
        };
        if let Err(error) = fsync(&directory) {
            warning = Some(format!("Saved, but folder synchronization failed: {error}"));
        }
        Ok(SaveOutcome { version, warning })
    })();
    let _ = unlinkat(&directory, &temporary, AtFlags::empty());
    result
}

#[cfg(not(unix))]
pub fn save(_: &ScanConfig, _: &Path, _: &FileVersion, _: &str) -> Result<SaveOutcome, String> {
    Err("Safe editing is currently supported only on Unix platforms".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, ScanConfig, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let config = ScanConfig::default_for_home(directory.path());
        let path = directory.path().join("external.md");
        fs::write(
            &path,
            "\u{feff}---\r\ntitle: Original\r\n---\r\n\r\n# café\r\n",
        )
        .unwrap();
        (directory, config, fs::canonicalize(path).unwrap())
    }
    #[test]
    fn full_sources_and_permissions_survive_atomic_saves() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_directory, config, path) = fixture();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let source = load(&config, &path).unwrap();
        let changed = source.text.replace("café", "edited");
        let outcome = save(&config, &path, &source.version, &changed).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(load(&config, &path).unwrap().version, outcome.version);
        save(&config, &path, &outcome.version, &(changed + "more\n")).unwrap();
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
    #[test]
    fn external_changes_deletion_and_atomic_replacement_are_conflicts() {
        for replacement in [false, true] {
            let (_directory, config, path) = fixture();
            let source = load(&config, &path).unwrap();
            if replacement {
                let other = path.with_extension("tmp");
                fs::write(&other, &source.text).unwrap();
                fs::rename(other, &path).unwrap();
            } else {
                fs::write(&path, "external change").unwrap();
            }
            assert!(
                save(&config, &path, &source.version, "draft")
                    .unwrap_err()
                    .contains("Conflict")
            );
            fs::remove_file(&path).unwrap();
            assert!(save(&config, &path, &source.version, "draft").is_err());
            assert!(!path.exists());
        }
    }
    #[test]
    fn root_exclusions_symlinks_and_size_limits_are_rechecked() {
        use std::os::unix::fs::symlink;
        let (directory, mut config, path) = fixture();
        let source = load(&config, &path).unwrap();
        config.max_file_size_bytes = 2;
        assert!(save(&config, &path, &source.version, "long draft").is_err());
        config.max_file_size_bytes = 2 * 1024 * 1024;
        config.excluded_globs = vec!["external.md".into()];
        assert!(save(&config, &path, &source.version, "draft").is_err());
        config.excluded_globs.clear();
        let outside = tempfile::tempdir().unwrap();
        config.roots = vec![outside.path().into()];
        assert!(save(&config, &path, &source.version, "draft").is_err());
        config.roots = vec![directory.path().into()];
        fs::remove_file(&path).unwrap();
        let target = outside.path().join("target.md");
        fs::write(&target, "untouched").unwrap();
        symlink(&target, &path).unwrap();
        assert!(save(&config, &path, &source.version, "draft").is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
    }
    #[test]
    fn final_recheck_detects_changes_during_the_temporary_write() {
        let (_directory, config, path) = fixture();
        let source = load(&config, &path).unwrap();
        let result = save_with_hook(&config, &path, &source.version, "draft", || {
            fs::write(&path, "external writer").unwrap();
        });
        assert!(result.unwrap_err().contains("Conflict"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "external writer");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn changed_directory_cannot_redirect_the_final_write() {
        use std::os::unix::fs::symlink;
        let (directory, config, original) = fixture();
        let inside = fs::canonicalize(directory.path()).unwrap().join("inside");
        fs::create_dir(&inside).unwrap();
        let path = inside.join("external.md");
        fs::rename(original, &path).unwrap();
        let source = load(&config, &path).unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("external.md"), "outside").unwrap();
        let moved = directory.path().join("moved");
        assert!(
            save_with_hook(&config, &path, &source.version, "draft", || {
                fs::rename(&inside, &moved).unwrap();
                symlink(outside.path(), &inside).unwrap();
            })
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("external.md")).unwrap(),
            "outside"
        );
        assert_eq!(
            fs::read_to_string(moved.join("external.md")).unwrap(),
            source.text
        );
        assert_eq!(fs::read_dir(moved).unwrap().count(), 1);
    }

    #[test]
    fn source_permissions_are_not_bypassed_by_directory_write_access() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_directory, config, path) = fixture();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        let source = load(&config, &path).unwrap();
        if fs::OpenOptions::new().write(true).open(&path).is_err() {
            assert!(save(&config, &path, &source.version, "draft").is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), source.text);
        }
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_acls_survive() {
        use std::process::Command;
        let (_directory, config, path) = fixture();
        assert!(
            Command::new("/bin/chmod")
                .args(["+a", "everyone allow read"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let acl = || {
            let output = Command::new("/bin/ls")
                .arg("-le")
                .arg(&path)
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let before = acl();
        assert!(before.contains("everyone"));
        let source = load(&config, &path).unwrap();
        save(&config, &path, &source.version, "changed").unwrap();
        assert_eq!(acl(), before);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_extended_attributes_survive() {
        use rustix::fs::{XattrFlags, fgetxattr, fsetxattr};
        let (_directory, config, path) = fixture();
        fsetxattr(
            fs::File::open(&path).unwrap(),
            "com.markdown-launcher.test",
            b"metadata",
            XattrFlags::empty(),
        )
        .unwrap();
        let source = load(&config, &path).unwrap();
        save(&config, &path, &source.version, "changed").unwrap();
        let mut bytes = [0u8; 64];
        let count = fgetxattr(
            fs::File::open(path).unwrap(),
            "com.markdown-launcher.test",
            &mut bytes,
        )
        .unwrap();
        assert_eq!(&bytes[..count], b"metadata");
    }
}
