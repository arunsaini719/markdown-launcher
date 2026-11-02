use std::{error::Error, fs, path::Path, process::ExitCode};

use gpui_kit::{
    ActivationPolicy, App, AppContext as _, Bounds, WindowBounds, WindowKind, WindowOptions, px,
    size,
};
use markdown_snippet_manager::{
    config::{AppConfig, AppPaths, ConfigError},
    desktop::{DesktopMenu, SingleInstanceGuard},
    index::{IndexDatabase, IndexWatchService},
    launcher::{Launcher, LauncherServices, RegisteredGlobalShortcut, init_launcher},
};

#[derive(Debug, PartialEq, Eq)]
enum StartupCommand {
    Launch,
    Help,
    Version,
    PrintPaths,
    CheckConfig,
    CheckIndex,
    Diagnostics,
    Shortcuts,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Markdown Launcher: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    match parse_startup_command(std::env::args().skip(1)).map_err(std::io::Error::other)? {
        StartupCommand::Help => {
            print!("{}", help_text());
            return Ok(());
        }
        StartupCommand::Version => {
            println!("Markdown Launcher {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        StartupCommand::PrintPaths => {
            let paths = AppPaths::discover()?;
            println!("Configuration: {}", paths.config_file.display());
            println!("Index: {}", paths.database_file.display());
            return Ok(());
        }
        StartupCommand::Shortcuts => {
            print!("{}", shortcuts_text());
            return Ok(());
        }
        StartupCommand::CheckConfig => {
            let paths = AppPaths::discover()?;
            validate_configuration(&paths.config_file)?;
            println!("Configuration is valid: {}", paths.config_file.display());
            return Ok(());
        }
        StartupCommand::CheckIndex => {
            let paths = AppPaths::discover()?;
            let health = IndexDatabase::check(&paths.database_file)?;
            println!(
                "Index is valid: schema {}, {} documents",
                health.schema_version, health.document_count
            );
            return Ok(());
        }
        StartupCommand::Diagnostics => {
            let paths = AppPaths::discover()?;
            print!("{}", diagnostics_text(&paths));
            return Ok(());
        }
        StartupCommand::Launch => {}
    }
    run_application()
}

fn parse_startup_command(args: impl IntoIterator<Item = String>) -> Result<StartupCommand, String> {
    let mut args = args.into_iter();
    let Some(argument) = args.next() else {
        return Ok(StartupCommand::Launch);
    };
    if args.next().is_some() {
        return Err("expected at most one option; run with --help for usage".into());
    }
    match argument.as_str() {
        "-h" | "--help" => Ok(StartupCommand::Help),
        "-V" | "--version" => Ok(StartupCommand::Version),
        "--print-paths" => Ok(StartupCommand::PrintPaths),
        "--shortcuts" => Ok(StartupCommand::Shortcuts),
        "--check-config" => Ok(StartupCommand::CheckConfig),
        "--check-index" => Ok(StartupCommand::CheckIndex),
        "--diagnostics" => Ok(StartupCommand::Diagnostics),
        _ => Err(format!(
            "unknown option '{argument}'; run with --help for usage"
        )),
    }
}

fn help_text() -> String {
    format!(
        concat!(
            "Markdown Launcher {version}\n",
            "A local-first, keyboard-driven Markdown launcher.\n\n",
            "Usage: markdown-snippet-manager [OPTIONS]\n\n",
            "Options:\n",
            "  -h, --help       Print help\n",
            "  -V, --version    Print version\n",
            "      --print-paths Show configuration and index paths\n",
            "      --shortcuts   Print the keyboard shortcut reference\n",
            "      --check-config Validate the configuration without launching\n",
            "      --check-index  Check index integrity without recovery or migration\n",
            "      --diagnostics  Print a privacy-conscious support report\n",
        ),
        version = env!("CARGO_PKG_VERSION")
    )
}

fn shortcuts_text() -> &'static str {
    concat!(
        "Keyboard shortcuts\n\n",
        "Launcher\n",
        "  Command/Ctrl+Shift+Space  Show or hide the launcher (default)\n",
        "  Command/Ctrl+Shift+R      Rebuild the search index\n",
        "  Command/Ctrl+N            Create a note\n",
        "  Command/Ctrl+E            Edit the selected note\n",
        "  Command/Ctrl+Shift+C      Copy the Markdown body and hide\n",
        "  Escape                   Return to search, clear search, or hide\n\n",
        "Search results\n",
        "  Up/Down                  Move the result selection\n",
        "  Ctrl+Home/End            Select the first or last result\n",
        "  Enter                    Focus the selected note preview\n\n",
        "Preview\n",
        "  Up/Down, Page Up/Down    Scroll the rendered note\n",
        "  Home/End, Command+Up/Down  Jump to the beginning or end\n",
        "  Command/Ctrl+A           Select all rendered text\n",
        "  Command/Ctrl+C           Copy selected rendered text\n\n",
        "Editor\n",
        "  Command/Ctrl+S           Save immediately\n",
        "  Command/Ctrl+A           Select all source text\n",
        "  Command/Ctrl+C           Copy selected source text\n",
        "  Escape                   Submit changes and return to preview\n\n",
        "New note\n",
        "  Enter                    Create the note when valid\n",
        "  Escape                   Cancel without creating a file\n\n",
        "macOS\n",
        "  Command+,                Open settings\n",
        "  Command+Q                Quit Markdown Launcher\n",
    )
}

fn validate_configuration(path: &Path) -> Result<AppConfig, Box<dyn Error>> {
    let mut config = AppConfig::load_for_editing(path)?;
    config.normalize_and_validate()?;
    Ok(config)
}

fn diagnostics_text(paths: &AppPaths) -> String {
    let configuration_status = match AppConfig::load_for_editing(&paths.config_file) {
        Ok(mut config) => match config.normalize_and_validate() {
            Ok(()) => "valid".to_string(),
            Err(ConfigError::Invalid(message)) => format!("invalid ({message})"),
            Err(_) => "invalid".to_string(),
        },
        Err(ConfigError::Read { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            "missing (will be created on launch)".to_string()
        }
        Err(ConfigError::Parse { .. }) => "invalid TOML".to_string(),
        Err(ConfigError::Read { source, .. }) => format!("unreadable ({:?})", source.kind()),
        Err(_) => "unavailable".to_string(),
    };
    let index_status = match fs::metadata(&paths.database_file) {
        Ok(metadata) if metadata.is_file() => format!("present ({} bytes)", metadata.len()),
        Ok(_) => "invalid file type".to_string(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "missing (will be built on launch)".to_string()
        }
        Err(error) => format!("unavailable ({:?})", error.kind()),
    };

    format!(
        concat!(
            "Markdown Launcher diagnostics\n",
            "Version: {version}\n",
            "Operating system: {os}\n",
            "Architecture: {architecture}\n",
            "Configuration: {configuration_status}\n",
            "Index: {index_status}\n",
        ),
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        architecture = std::env::consts::ARCH,
        configuration_status = configuration_status,
        index_status = index_status,
    )
}

fn run_application() -> Result<(), Box<dyn Error>> {
    let paths = AppPaths::discover()?;
    let lock_file = paths.config_file.with_file_name("application.lock");
    let Some(single_instance) =
        SingleInstanceGuard::acquire(&lock_file).map_err(std::io::Error::other)?
    else {
        eprintln!("Markdown Launcher is already running");
        return Ok(());
    };
    let loaded_config = AppConfig::load_or_default(&paths)?;
    let config_file = paths.config_file.clone();
    let config = loaded_config.config;
    let max_file_size_bytes = config.scan.max_file_size_bytes;
    let scan_config = config.scan.clone();
    let global_shortcut_setting = config.global_shortcut;
    let (watch_service, watch_error) =
        match IndexWatchService::start(config.scan, paths.database_file.clone()) {
            Ok(service) => (Some(service), None),
            Err(error) => (None, Some(error.to_string())),
        };
    let startup_error = combine_warnings(loaded_config.warning, watch_error);
    let database_path = paths.database_file;

    // SquarePen is outside GPUI's default component icon bundle.
    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    application.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        init_launcher(cx);
        let (global_shortcut, shortcut_error) =
            match RegisteredGlobalShortcut::register(&global_shortcut_setting) {
                Ok(shortcut) => (Some(shortcut), None),
                Err(error) => (None, Some(format!("global shortcut unavailable: {error}"))),
            };
        let startup_error = combine_warnings(startup_error, shortcut_error);
        let (desktop_menu, menu_error) = match DesktopMenu::create() {
            Ok(menu) => (menu, None),
            Err(error) => (None, Some(error)),
        };
        let startup_error = combine_warnings(startup_error, menu_error);
        let start_hidden = desktop_menu
            .as_ref()
            .is_some_and(DesktopMenu::launch_at_login_enabled);

        let bounds = Bounds::centered(None, size(px(1040.0), px(640.0)), cx);
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(800.0), px(480.0))),
                titlebar: None,
                focus: !start_hidden,
                show: !start_hidden,
                kind: WindowKind::PopUp,
                is_resizable: false,
                is_minimizable: false,
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    Launcher::new(
                        database_path,
                        max_file_size_bytes,
                        LauncherServices {
                            watch_service,
                            global_shortcut,
                            desktop_menu,
                            single_instance: Some(single_instance),
                            config_file: Some(config_file),
                            scan_config: Some(scan_config),
                        },
                        startup_error,
                        window,
                        cx,
                    )
                })
            },
        );

        if let Err(error) = result {
            eprintln!("failed to open the launcher window: {error}");
            cx.quit();
        } else {
            cx.set_activation_policy(ActivationPolicy::Accessory);
        }
    });

    Ok(())
}

fn combine_warnings(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}; {second}")),
        (Some(message), None) | (None, Some(message)) => Some(message),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_informational_startup_options_without_launching_the_app() {
        assert_eq!(
            parse_startup_command(Vec::<String>::new()).unwrap(),
            StartupCommand::Launch
        );
        for option in ["-h", "--help"] {
            assert_eq!(
                parse_startup_command(vec![option.into()]).unwrap(),
                StartupCommand::Help
            );
        }
        for option in ["-V", "--version"] {
            assert_eq!(
                parse_startup_command(vec![option.into()]).unwrap(),
                StartupCommand::Version
            );
        }
        assert_eq!(
            parse_startup_command(vec!["--print-paths".into()]).unwrap(),
            StartupCommand::PrintPaths
        );
        assert_eq!(
            parse_startup_command(vec!["--shortcuts".into()]).unwrap(),
            StartupCommand::Shortcuts
        );
        assert_eq!(
            parse_startup_command(vec!["--check-config".into()]).unwrap(),
            StartupCommand::CheckConfig
        );
        assert_eq!(
            parse_startup_command(vec!["--check-index".into()]).unwrap(),
            StartupCommand::CheckIndex
        );
        assert_eq!(
            parse_startup_command(vec!["--diagnostics".into()]).unwrap(),
            StartupCommand::Diagnostics
        );
    }

    #[test]
    fn rejects_unknown_or_multiple_startup_options() {
        assert!(
            parse_startup_command(vec!["--unknown".into()])
                .unwrap_err()
                .contains("unknown option")
        );
        assert!(
            parse_startup_command(vec!["--help".into(), "--version".into()])
                .unwrap_err()
                .contains("at most one")
        );
    }

    #[test]
    fn help_includes_the_binary_name_and_package_version() {
        let help = help_text();
        assert!(help.contains("markdown-snippet-manager [OPTIONS]"));
        assert!(help.contains(env!("CARGO_PKG_VERSION")));
        assert!(help.contains("--print-paths"));
        assert!(help.contains("--shortcuts"));
        assert!(help.contains("--check-config"));
        assert!(help.contains("--check-index"));
        assert!(help.contains("--diagnostics"));
    }

    #[test]
    fn shortcut_reference_covers_each_keyboard_context() {
        let shortcuts = shortcuts_text();

        for context in [
            "Launcher",
            "Search results",
            "Preview",
            "Editor",
            "New note",
            "macOS",
        ] {
            assert!(shortcuts.contains(context));
        }
        assert!(shortcuts.contains("Command/Ctrl+Shift+Space"));
        assert!(shortcuts.contains("Ctrl+Home/End"));
        assert!(shortcuts.contains("Command/Ctrl+S"));
    }

    #[test]
    fn validates_configuration_without_creating_or_rewriting_it() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_file = directory.path().join("config.toml");
        let config = AppConfig::default_for_home(directory.path());
        config.save(&config_file).expect("save valid config");
        let original = std::fs::read_to_string(&config_file).expect("read valid config");

        assert_eq!(
            validate_configuration(&config_file)
                .expect("validate config")
                .scan
                .roots,
            vec![directory.path().to_path_buf()]
        );
        assert_eq!(
            std::fs::read_to_string(&config_file).expect("reread valid config"),
            original
        );

        std::fs::write(&config_file, "unknown = true\n").expect("write invalid config");
        assert!(validate_configuration(&config_file).is_err());
        assert_eq!(
            std::fs::read_to_string(&config_file).expect("reread invalid config"),
            "unknown = true\n"
        );
    }

    #[test]
    fn diagnostics_report_state_without_exposing_application_paths() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let paths = AppPaths {
            config_file: directory.path().join("private/config.toml"),
            database_file: directory.path().join("private/index.sqlite3"),
        };

        let missing = diagnostics_text(&paths);
        assert!(missing.contains("Configuration: missing"));
        assert!(missing.contains("Index: missing"));
        assert!(!missing.contains(&directory.path().display().to_string()));

        let config = AppConfig::default_for_home(directory.path());
        config
            .save(&paths.config_file)
            .expect("save diagnostic config");
        std::fs::write(&paths.database_file, b"index").expect("write diagnostic index");
        let present = diagnostics_text(&paths);
        assert!(present.contains("Configuration: valid"));
        assert!(present.contains("Index: present (5 bytes)"));
        assert!(!present.contains(&directory.path().display().to_string()));

        std::fs::write(&paths.config_file, "unknown = true\n").expect("write invalid config");
        assert!(diagnostics_text(&paths).contains("Configuration: invalid TOML"));
    }

    #[test]
    fn workspace_and_component_icons_have_renderable_bundled_artwork() {
        use gpui_kit::{
            AssetSource as _, SvgRenderer,
            assets::{AllAssets, IconName},
        };
        use std::sync::Arc;

        let assets = Arc::new(AllAssets);
        let renderer = SvgRenderer::new(assets.clone());
        for icon in [
            IconName::Plus,
            IconName::SquarePen,
            IconName::Copy,
            IconName::Check,
            IconName::ChevronDown,
            IconName::X,
        ] {
            let path = icon.path();
            let bytes = assets
                .load(&path)
                .unwrap()
                .unwrap_or_else(|| panic!("Missing icon: {path}"));
            if !cfg!(debug_assertions) {
                assert!(
                    matches!(&bytes, std::borrow::Cow::Borrowed(_)),
                    "Icon is not embedded: {path}"
                );
            }
            let image = renderer.render_single_frame(&bytes, 1.0).unwrap();
            assert!(
                image
                    .as_bytes(0)
                    .unwrap()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] > 0),
                "Blank icon: {path}"
            );
        }
    }

    #[test]
    fn preserves_all_startup_warnings() {
        assert_eq!(
            combine_warnings(Some("config".into()), Some("shortcut".into())).as_deref(),
            Some("config; shortcut")
        );
    }
}
