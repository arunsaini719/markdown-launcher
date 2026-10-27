use std::error::Error;

use gpui_kit::{
    ActivationPolicy, App, AppContext as _, Bounds, WindowBounds, WindowKind, WindowOptions, px,
    size,
};
use markdown_snippet_manager::{
    config::{AppConfig, AppPaths},
    desktop::{DesktopMenu, SingleInstanceGuard},
    index::IndexWatchService,
    launcher::{Launcher, LauncherServices, RegisteredGlobalShortcut, init_launcher},
};

fn main() {
    if let Err(error) = run() {
        eprintln!("failed to start Markdown Launcher: {error}");
    }
}

fn run() -> Result<(), Box<dyn Error>> {
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
