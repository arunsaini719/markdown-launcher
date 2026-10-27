use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use directories::{BaseDirs, ProjectDirs};
use globset::Glob;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const APP_QUALIFIER: &str = "com";
const APP_ORGANIZATION: &str = "markdown-launcher";
const APP_NAME: &str = "MarkdownSnippetManager";
const CONFIG_FILE_NAME: &str = "config.toml";
const DATABASE_FILE_NAME: &str = "index.sqlite3";

const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_GLOBAL_SHORTCUT: &str = "cmdorctrl+shift+Space";
const DEFAULT_EXTENSIONS: &[&str] = &["md", "markdown"];
const DEFAULT_EXCLUDED_DIRECTORY_NAMES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".cache",
    ".Trash",
    "node_modules",
    "target",
];
const DEFAULT_EXCLUDED_GLOBS: &[&str] = &["Library/Caches", "Library/Caches/**"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_file: PathBuf,
    pub database_file: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self, ConfigError> {
        let project_dirs = ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
            .ok_or(ConfigError::ApplicationDirectoriesUnavailable)?;

        Ok(Self {
            config_file: project_dirs.config_dir().join(CONFIG_FILE_NAME),
            database_file: project_dirs.data_dir().join(DATABASE_FILE_NAME),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub global_shortcut: String,
    pub scan: ScanConfig,
}

impl AppConfig {
    pub fn default_for_home(home: impl Into<PathBuf>) -> Self {
        Self {
            global_shortcut: DEFAULT_GLOBAL_SHORTCUT.into(),
            scan: ScanConfig::default_for_home(home),
        }
    }

    pub fn load_or_create(paths: &AppPaths) -> Result<Self, ConfigError> {
        let home = BaseDirs::new()
            .map(|directories| directories.home_dir().to_path_buf())
            .ok_or(ConfigError::HomeDirectoryUnavailable)?;

        Self::load_or_create_for_home(paths, home)
    }

    pub fn load_or_default(paths: &AppPaths) -> Result<LoadedConfig, ConfigError> {
        let home = BaseDirs::new()
            .map(|directories| directories.home_dir().to_path_buf())
            .ok_or(ConfigError::HomeDirectoryUnavailable)?;

        let mut config = if paths.config_file.exists() {
            Self::read_config(&paths.config_file)?
        } else {
            Self::load_or_create_for_home(paths, &home)?
        };
        config.scan.normalize_and_validate()?;
        let warning = match config.normalize_shortcut() {
            Ok(()) => None,
            Err(error) => {
                config.global_shortcut = DEFAULT_GLOBAL_SHORTCUT.into();
                Some(format!("default global shortcut is active: {error}"))
            }
        };
        Ok(LoadedConfig { config, warning })
    }

    pub fn load_or_create_for_home(
        paths: &AppPaths,
        home: impl Into<PathBuf>,
    ) -> Result<Self, ConfigError> {
        if paths.config_file.exists() {
            let mut config = Self::read_config(&paths.config_file)?;
            config.normalize_and_validate()?;
            return Ok(config);
        }

        let mut config = Self::default_for_home(home);
        config.normalize_and_validate()?;
        config.save(&paths.config_file)?;
        Ok(config)
    }

    pub fn load_for_editing(path: &Path) -> Result<Self, ConfigError> {
        Self::read_config(path)
    }

    fn read_config(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&contents).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let mut config = self.clone();
        config.normalize_and_validate()?;

        let contents = toml::to_string_pretty(&config).map_err(ConfigError::Serialize)?;
        let parent = path.parent().ok_or_else(|| {
            ConfigError::Invalid("the configuration path has no parent directory".into())
        })?;
        fs::create_dir_all(parent).map_err(|source| ConfigError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;

        let temporary_path = path.with_extension("toml.tmp");
        let write_result = (|| {
            let mut file =
                fs::File::create(&temporary_path).map_err(|source| ConfigError::Write {
                    path: temporary_path.clone(),
                    source,
                })?;
            file.write_all(contents.as_bytes())
                .map_err(|source| ConfigError::Write {
                    path: temporary_path.clone(),
                    source,
                })?;
            file.sync_all().map_err(|source| ConfigError::Write {
                path: temporary_path.clone(),
                source,
            })?;
            fs::rename(&temporary_path, path).map_err(|source| ConfigError::Write {
                path: path.to_path_buf(),
                source,
            })
        })();

        if write_result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }

        write_result
    }

    pub fn normalize_and_validate(&mut self) -> Result<(), ConfigError> {
        self.normalize_shortcut()?;
        self.scan.normalize_and_validate()
    }

    fn normalize_shortcut(&mut self) -> Result<(), ConfigError> {
        self.global_shortcut = self.global_shortcut.trim().to_string();
        self.global_shortcut
            .parse::<global_hotkey::hotkey::HotKey>()
            .map_err(|error| ConfigError::Invalid(format!("invalid global shortcut: {error}")))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: AppConfig,
    pub warning: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let home = BaseDirs::new()
            .map(|directories| directories.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        Self::default_for_home(home)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScanConfig {
    pub roots: Vec<PathBuf>,
    pub extensions: Vec<String>,
    pub excluded_directory_names: Vec<String>,
    pub excluded_globs: Vec<String>,
    pub max_file_size_bytes: u64,
    pub follow_symbolic_links: bool,
}

impl ScanConfig {
    pub fn default_for_home(home: impl Into<PathBuf>) -> Self {
        Self {
            roots: vec![home.into()],
            extensions: DEFAULT_EXTENSIONS
                .iter()
                .map(|extension| (*extension).to_string())
                .collect(),
            excluded_directory_names: DEFAULT_EXCLUDED_DIRECTORY_NAMES
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            excluded_globs: DEFAULT_EXCLUDED_GLOBS
                .iter()
                .map(|pattern| (*pattern).to_string())
                .collect(),
            max_file_size_bytes: DEFAULT_MAX_FILE_SIZE_BYTES,
            follow_symbolic_links: false,
        }
    }

    pub fn normalize_and_validate(&mut self) -> Result<(), ConfigError> {
        deduplicate(&mut self.roots);
        if self.roots.is_empty() || self.roots.iter().any(|root| root.as_os_str().is_empty()) {
            return Err(ConfigError::Invalid(
                "at least one non-empty scan root is required".into(),
            ));
        }

        self.extensions = self
            .extensions
            .drain(..)
            .map(|extension| extension.trim().trim_start_matches('.').to_lowercase())
            .collect();
        deduplicate(&mut self.extensions);
        if self.extensions.is_empty() || self.extensions.iter().any(String::is_empty) {
            return Err(ConfigError::Invalid(
                "at least one non-empty file extension is required".into(),
            ));
        }

        self.excluded_directory_names = self
            .excluded_directory_names
            .drain(..)
            .map(|name| name.trim().to_string())
            .collect();
        deduplicate(&mut self.excluded_directory_names);
        if self.excluded_directory_names.iter().any(|name| {
            name.is_empty()
                || name.contains(std::path::MAIN_SEPARATOR)
                || (std::path::MAIN_SEPARATOR != '/' && name.contains('/'))
        }) {
            return Err(ConfigError::Invalid(
                "excluded directory names must be individual directory names".into(),
            ));
        }

        self.excluded_globs = self
            .excluded_globs
            .drain(..)
            .map(|pattern| pattern.trim().to_string())
            .collect();
        deduplicate(&mut self.excluded_globs);
        for pattern in &self.excluded_globs {
            if pattern.is_empty() {
                return Err(ConfigError::Invalid(
                    "excluded glob patterns cannot be empty".into(),
                ));
            }
            Glob::new(pattern).map_err(|error| {
                ConfigError::Invalid(format!("invalid exclusion glob `{pattern}`: {error}"))
            })?;
        }

        if self.max_file_size_bytes == 0 {
            return Err(ConfigError::Invalid(
                "maximum indexed file size must be greater than zero".into(),
            ));
        }

        Ok(())
    }
}

impl Default for ScanConfig {
    fn default() -> Self {
        AppConfig::default().scan
    }
}

fn deduplicate<T>(values: &mut Vec<T>)
where
    T: Eq + std::hash::Hash + Clone,
{
    let mut seen = HashSet::with_capacity(values.len());
    values.retain(|value| seen.insert(value.clone()));
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("the operating system application directories are unavailable")]
    ApplicationDirectoriesUnavailable,
    #[error("the operating system home directory is unavailable")]
    HomeDirectoryUnavailable,
    #[error("failed to read configuration at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse configuration at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to serialize configuration: {0}")]
    Serialize(#[source] toml::ser::Error),
    #[error("failed to create configuration directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write configuration at {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_provided_home_with_safe_scan_settings() {
        let home = PathBuf::from("/example/home");
        let config = AppConfig::default_for_home(&home);

        assert_eq!(config.scan.roots, vec![home]);
        assert_eq!(config.global_shortcut, DEFAULT_GLOBAL_SHORTCUT);
        assert_eq!(config.scan.extensions, ["md", "markdown"]);
        assert!(!config.scan.follow_symbolic_links);
        assert!(
            config
                .scan
                .excluded_directory_names
                .contains(&".git".to_string())
        );
        assert!(config.scan.max_file_size_bytes > 0);
    }

    #[test]
    fn creates_and_reloads_a_default_configuration() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let paths = AppPaths {
            config_file: temporary_directory.path().join("config/config.toml"),
            database_file: temporary_directory.path().join("data/index.sqlite3"),
        };
        let home = temporary_directory.path().join("home");

        let created = AppConfig::load_or_create_for_home(&paths, &home).expect("create config");
        let loaded =
            AppConfig::load_or_create_for_home(&paths, "/different/home").expect("load config");

        assert_eq!(created, loaded);
        assert_eq!(loaded.scan.roots, vec![home]);
        assert!(paths.config_file.is_file());
    }

    #[test]
    fn malformed_configuration_stops_startup_without_overwriting_the_file() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let config_file = temporary_directory.path().join("config.toml");
        fs::write(&config_file, "not valid = [toml").expect("write invalid config");
        let paths = AppPaths {
            config_file: config_file.clone(),
            database_file: temporary_directory.path().join("index.sqlite3"),
        };

        let error = AppConfig::load_or_default(&paths).expect_err("unsafe config must fail");

        assert!(matches!(error, ConfigError::Parse { .. }));
        assert_eq!(
            fs::read_to_string(config_file).expect("read original config"),
            "not valid = [toml"
        );
    }

    #[test]
    fn invalid_shortcut_preserves_the_explicit_scan_scope() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let paths = AppPaths {
            config_file: temporary_directory.path().join("config.toml"),
            database_file: temporary_directory.path().join("index.sqlite3"),
        };
        let original = "global_shortcut = 'invalid-key'\n[scan]\nroots = ['/explicit/notes']\nexcluded_globs = ['private/**']\nmax_file_size_bytes = 1234\n";
        fs::write(&paths.config_file, original).expect("config");
        let loaded = AppConfig::load_or_default(&paths).expect("recover shortcut");
        assert_eq!(loaded.config.global_shortcut, DEFAULT_GLOBAL_SHORTCUT);
        assert_eq!(
            loaded.config.scan.roots,
            vec![PathBuf::from("/explicit/notes")]
        );
        assert_eq!(loaded.config.scan.excluded_globs, ["private/**"]);
        assert_eq!(loaded.config.scan.max_file_size_bytes, 1234);
        assert!(loaded.warning.is_some());
        assert_eq!(
            fs::read_to_string(&paths.config_file).expect("original"),
            original
        );
    }

    #[test]
    fn invalid_scan_settings_never_fall_back_to_home_scanning() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let paths = AppPaths {
            config_file: temporary_directory.path().join("config.toml"),
            database_file: temporary_directory.path().join("index.sqlite3"),
        };
        for contents in [
            "[scan]\nroots = []\n",
            "[scan]\nroots = ['/explicit/notes']\nexcluded_globs = ['[invalid']\n",
            "unknown = true\n[scan]\nroots = ['/explicit/notes']\n",
        ] {
            fs::write(&paths.config_file, contents).expect("invalid config");
            assert!(AppConfig::load_or_default(&paths).is_err());
            assert_eq!(
                fs::read_to_string(&paths.config_file).expect("original"),
                contents
            );
            assert!(!paths.database_file.exists());
        }
    }

    #[test]
    fn normalizes_extensions_and_removes_duplicates() {
        let mut config = AppConfig::default_for_home("/example/home");
        config.scan.extensions = vec![".MD".into(), " markdown ".into(), "md".into()];

        config.normalize_and_validate().expect("valid config");

        assert_eq!(config.scan.extensions, ["md", "markdown"]);
    }

    #[test]
    fn rejects_invalid_global_shortcuts() {
        let mut config = AppConfig::default_for_home("/example/home");
        config.global_shortcut = "shift+not-a-key".into();

        let error = config
            .normalize_and_validate()
            .expect_err("invalid shortcut should fail");

        assert!(error.to_string().contains("invalid global shortcut"));
    }

    #[test]
    fn rejects_invalid_scan_settings() {
        let mut config = AppConfig::default_for_home("/example/home");
        config.scan.excluded_globs = vec!["[invalid".into()];

        let error = config
            .normalize_and_validate()
            .expect_err("invalid glob should fail");

        assert!(error.to_string().contains("invalid exclusion glob"));
    }

    #[test]
    fn rejects_unknown_configuration_fields() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let config_file = temporary_directory.path().join("config.toml");
        fs::write(
            &config_file,
            r#"
                unknown = true

                [scan]
                roots = ["/example/home"]
            "#,
        )
        .expect("write config");
        let paths = AppPaths {
            config_file,
            database_file: temporary_directory.path().join("index.sqlite3"),
        };

        let error = AppConfig::load_or_create_for_home(&paths, "/example/home")
            .expect_err("unknown field should fail");

        assert!(matches!(error, ConfigError::Parse { .. }));
    }
}
