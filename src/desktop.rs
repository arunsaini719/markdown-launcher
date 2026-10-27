use std::path::Path;

#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopMenuAction {
    OpenLauncher,
    OpenSettings,
    RebuildIndex,
    ToggleLaunchAtLogin,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchAtLoginChange {
    Enabled,
    Disabled,
    ApprovalRequired,
}

const OPEN_LAUNCHER_ID: &str = "open-launcher";
const OPEN_SETTINGS_ID: &str = "open-settings";
const REBUILD_INDEX_ID: &str = "rebuild-index";
const TOGGLE_LAUNCH_AT_LOGIN_ID: &str = "toggle-launch-at-login";
const QUIT_ID: &str = "quit";

#[cfg(target_os = "macos")]
const LOGIN_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

pub struct SingleInstanceGuard {
    #[cfg(target_os = "macos")]
    _instance: single_instance::SingleInstance,
}

impl SingleInstanceGuard {
    pub fn acquire(lock_file: &Path) -> Result<Option<Self>, String> {
        #[cfg(target_os = "macos")]
        {
            let parent = lock_file
                .parent()
                .ok_or_else(|| "the application lock path has no parent directory".to_string())?;
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create the application lock directory {}: {error}",
                    parent.display()
                )
            })?;
            let instance = single_instance::SingleInstance::new(&lock_file.to_string_lossy())
                .map_err(|error| format!("failed to acquire the application lock: {error}"))?;
            if !instance.is_single() {
                return Ok(None);
            }
            Ok(Some(Self {
                _instance: instance,
            }))
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = lock_file;
            Ok(Some(Self {}))
        }
    }
}

pub struct DesktopMenu {
    #[cfg(target_os = "macos")]
    _tray_icon: tray_icon::TrayIcon,
    #[cfg(target_os = "macos")]
    launch_at_login_item: tray_icon::menu::CheckMenuItem,
    #[cfg(target_os = "macos")]
    login_service: smappservice_rs::AppService,
    #[cfg(target_os = "macos")]
    can_register_login_item: bool,
    #[cfg(target_os = "macos")]
    last_login_status_refresh: Instant,
}

impl DesktopMenu {
    pub fn create() -> Result<Option<Self>, String> {
        #[cfg(target_os = "macos")]
        {
            use smappservice_rs::{AppService, ServiceType};
            use tray_icon::{
                Icon, TrayIconBuilder,
                menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
            };

            let login_service = AppService::new(ServiceType::MainApp);
            let can_register_login_item = application_is_installed();
            let login_presentation =
                login_item_presentation(login_service.status(), can_register_login_item);
            let open = MenuItem::with_id(OPEN_LAUNCHER_ID, "Open Launcher", true, None);
            let settings = MenuItem::with_id(OPEN_SETTINGS_ID, "Settings…", true, None);
            let rebuild = MenuItem::with_id(REBUILD_INDEX_ID, "Rebuild Index", true, None);
            let action_separator = PredefinedMenuItem::separator();
            let launch_at_login_item = CheckMenuItem::with_id(
                TOGGLE_LAUNCH_AT_LOGIN_ID,
                login_presentation.label,
                login_presentation.enabled,
                login_presentation.checked,
                None,
            );
            let quit_separator = PredefinedMenuItem::separator();
            let quit = MenuItem::with_id(QUIT_ID, "Quit Markdown Launcher", true, None);
            let menu = Menu::with_items(&[
                &open,
                &settings,
                &rebuild,
                &action_separator,
                &launch_at_login_item,
                &quit_separator,
                &quit,
            ])
            .map_err(|error| format!("failed to create the menu bar menu: {error}"))?;
            let icon = Icon::from_rgba(menu_bar_icon_rgba(32), 32, 32)
                .map_err(|error| format!("failed to create the menu bar icon: {error}"))?;
            let tray_icon = TrayIconBuilder::new()
                .with_tooltip("Markdown Launcher")
                .with_autosave_name("MarkdownLauncherStatusItem")
                .with_menu(Box::new(menu))
                .with_icon_templated(icon)
                .build()
                .map_err(|error| format!("failed to create the menu bar item: {error}"))?;

            Ok(Some(Self {
                _tray_icon: tray_icon,
                launch_at_login_item,
                login_service,
                can_register_login_item,
                last_login_status_refresh: Instant::now(),
            }))
        }

        #[cfg(not(target_os = "macos"))]
        {
            Ok(None)
        }
    }

    pub fn launch_at_login_enabled(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            use smappservice_rs::ServiceStatus;

            self.login_service.status() == ServiceStatus::Enabled
        }

        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn refresh_launch_at_login_status(&mut self) {
        #[cfg(target_os = "macos")]
        {
            if self.last_login_status_refresh.elapsed() < LOGIN_STATUS_REFRESH_INTERVAL {
                return;
            }
            self.update_login_item();
        }
    }

    pub fn toggle_launch_at_login(&mut self) -> Result<LaunchAtLoginChange, String> {
        #[cfg(target_os = "macos")]
        {
            use smappservice_rs::{AppService, ServiceStatus};

            let result = match self.login_service.status() {
                ServiceStatus::Enabled => self
                    .login_service
                    .unregister()
                    .map(|()| LaunchAtLoginChange::Disabled)
                    .map_err(|error| format!("could not disable Launch at Login: {error}")),
                ServiceStatus::NotRegistered if self.can_register_login_item => self
                    .login_service
                    .register()
                    .map(|()| LaunchAtLoginChange::Enabled)
                    .map_err(|error| format!("could not enable Launch at Login: {error}")),
                ServiceStatus::NotRegistered => Err(
                    "Install Markdown Launcher in an Applications directory before enabling Launch at Login"
                        .to_string(),
                ),
                ServiceStatus::RequiresApproval => {
                    AppService::open_system_settings_login_items();
                    Ok(LaunchAtLoginChange::ApprovalRequired)
                }
                ServiceStatus::NotFound => Err(
                    "Launch at Login is unavailable; install and open the application from /Applications"
                        .to_string(),
                ),
            };

            self.update_login_item();
            if self.login_service.status() == ServiceStatus::RequiresApproval {
                AppService::open_system_settings_login_items();
                return Ok(LaunchAtLoginChange::ApprovalRequired);
            }
            result
        }

        #[cfg(not(target_os = "macos"))]
        {
            Err("Launch at Login is available only on macOS".to_string())
        }
    }

    #[cfg(target_os = "macos")]
    fn update_login_item(&mut self) {
        let presentation =
            login_item_presentation(self.login_service.status(), self.can_register_login_item);
        self.launch_at_login_item.set_text(presentation.label);
        self.launch_at_login_item.set_enabled(presentation.enabled);
        self.launch_at_login_item.set_checked(presentation.checked);
        self.last_login_status_refresh = Instant::now();
    }

    pub fn next_action(&self) -> Option<DesktopMenuAction> {
        #[cfg(target_os = "macos")]
        {
            tray_icon::menu::MenuEvent::receiver()
                .try_recv()
                .ok()
                .and_then(|event| action_for_menu_id(event.id().as_ref()))
        }

        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }
}

fn action_for_menu_id(id: &str) -> Option<DesktopMenuAction> {
    match id {
        OPEN_LAUNCHER_ID => Some(DesktopMenuAction::OpenLauncher),
        OPEN_SETTINGS_ID => Some(DesktopMenuAction::OpenSettings),
        REBUILD_INDEX_ID => Some(DesktopMenuAction::RebuildIndex),
        TOGGLE_LAUNCH_AT_LOGIN_ID => Some(DesktopMenuAction::ToggleLaunchAtLogin),
        QUIT_ID => Some(DesktopMenuAction::Quit),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn application_is_installed() -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let user_applications =
        directories::BaseDirs::new().map(|directories| directories.home_dir().join("Applications"));
    executable_is_in_applications(
        &executable,
        Path::new("/Applications"),
        user_applications.as_deref(),
    )
}

#[cfg(target_os = "macos")]
fn executable_is_in_applications(
    executable: &Path,
    system_applications: &Path,
    user_applications: Option<&Path>,
) -> bool {
    let Some(bundle) = executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
    else {
        return false;
    };
    bundle.starts_with(system_applications)
        || user_applications.is_some_and(|applications| bundle.starts_with(applications))
}

#[cfg(target_os = "macos")]
struct LoginItemPresentation {
    label: &'static str,
    enabled: bool,
    checked: bool,
}

#[cfg(target_os = "macos")]
fn login_item_presentation(
    status: smappservice_rs::ServiceStatus,
    can_register: bool,
) -> LoginItemPresentation {
    use smappservice_rs::ServiceStatus;

    match status {
        ServiceStatus::Enabled => LoginItemPresentation {
            label: "Launch at Login",
            enabled: true,
            checked: true,
        },
        ServiceStatus::NotRegistered if can_register => LoginItemPresentation {
            label: "Launch at Login",
            enabled: true,
            checked: false,
        },
        ServiceStatus::NotRegistered => LoginItemPresentation {
            label: "Launch at Login (Install in Applications)",
            enabled: false,
            checked: false,
        },
        ServiceStatus::RequiresApproval => LoginItemPresentation {
            label: "Launch at Login (Approval Required)",
            enabled: true,
            checked: false,
        },
        ServiceStatus::NotFound => LoginItemPresentation {
            label: "Launch at Login (Unavailable)",
            enabled: false,
            checked: false,
        },
    }
}

#[cfg(target_os = "macos")]
fn menu_bar_icon_rgba(size: u32) -> Vec<u8> {
    fn distance_to_segment(x: f32, y: f32, start: (f32, f32), end: (f32, f32)) -> f32 {
        let segment = (end.0 - start.0, end.1 - start.1);
        let offset = (x - start.0, y - start.1);
        let length_squared = segment.0 * segment.0 + segment.1 * segment.1;
        let projection = if length_squared == 0.0 {
            0.0
        } else {
            ((offset.0 * segment.0 + offset.1 * segment.1) / length_squared).clamp(0.0, 1.0)
        };
        let nearest = (
            start.0 + projection * segment.0,
            start.1 + projection * segment.1,
        );
        (x - nearest.0).hypot(y - nearest.1)
    }

    let scale = size as f32 / 32.0;
    let strokes = [
        ((5.0, 7.0), (5.0, 18.0)),
        ((5.0, 7.0), (16.0, 16.0)),
        ((16.0, 16.0), (27.0, 7.0)),
        ((27.0, 7.0), (27.0, 18.0)),
        ((16.0, 18.0), (16.0, 27.0)),
        ((16.0, 27.0), (11.0, 22.0)),
        ((16.0, 27.0), (21.0, 22.0)),
    ];
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let point = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
            let alpha = if strokes
                .iter()
                .any(|&(start, end)| distance_to_segment(point.0, point.1, start, end) <= 1.8)
            {
                255
            } else {
                0
            };
            rgba.extend_from_slice(&[0, 0, 0, alpha]);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_menu_item_ids_to_actions() {
        assert_eq!(
            action_for_menu_id(OPEN_LAUNCHER_ID),
            Some(DesktopMenuAction::OpenLauncher)
        );
        assert_eq!(
            action_for_menu_id(OPEN_SETTINGS_ID),
            Some(DesktopMenuAction::OpenSettings)
        );
        assert_eq!(
            action_for_menu_id(REBUILD_INDEX_ID),
            Some(DesktopMenuAction::RebuildIndex)
        );
        assert_eq!(
            action_for_menu_id(TOGGLE_LAUNCH_AT_LOGIN_ID),
            Some(DesktopMenuAction::ToggleLaunchAtLogin)
        );
        assert_eq!(action_for_menu_id(QUIT_ID), Some(DesktopMenuAction::Quit));
        assert_eq!(action_for_menu_id("unknown"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn login_item_presentation_defaults_to_unchecked() {
        use smappservice_rs::ServiceStatus;

        let disabled = login_item_presentation(ServiceStatus::NotRegistered, true);
        assert!(disabled.enabled);
        assert!(!disabled.checked);
        assert_eq!(disabled.label, "Launch at Login");

        let enabled = login_item_presentation(ServiceStatus::Enabled, true);
        assert!(enabled.enabled);
        assert!(enabled.checked);

        let approval = login_item_presentation(ServiceStatus::RequiresApproval, true);
        assert!(approval.enabled);
        assert!(!approval.checked);
        assert!(approval.label.contains("Approval Required"));

        let unavailable = login_item_presentation(ServiceStatus::NotFound, true);
        assert!(!unavailable.enabled);
        assert!(!unavailable.checked);

        let not_installed = login_item_presentation(ServiceStatus::NotRegistered, false);
        assert!(!not_installed.enabled);
        assert!(!not_installed.checked);
        assert!(not_installed.label.contains("Install in Applications"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn recognizes_installed_application_bundles() {
        let system_applications = Path::new("/Applications");
        let user_applications = Path::new("/Users/example/Applications");
        assert!(executable_is_in_applications(
            Path::new(
                "/Applications/Markdown Launcher.app/Contents/MacOS/markdown-snippet-manager"
            ),
            system_applications,
            Some(user_applications),
        ));
        assert!(executable_is_in_applications(
            Path::new(
                "/Users/example/Applications/Markdown Launcher.app/Contents/MacOS/markdown-snippet-manager"
            ),
            system_applications,
            Some(user_applications),
        ));
        assert!(!executable_is_in_applications(
            Path::new("/workspace/target/release/markdown-snippet-manager"),
            system_applications,
            Some(user_applications),
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn menu_bar_icon_has_visible_and_transparent_pixels() {
        let rgba = menu_bar_icon_rgba(32);
        assert_eq!(rgba.len(), 32 * 32 * 4);
        let (pixels, remainder) = rgba.as_chunks::<4>();
        assert!(remainder.is_empty());
        assert!(pixels.iter().any(|pixel| pixel[3] == 255));
        assert!(pixels.iter().any(|pixel| pixel[3] == 0));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn application_lock_prevents_a_second_instance() {
        let directory = tempfile::tempdir().expect("temp directory");
        let lock_file = directory.path().join("application.lock");
        let first = SingleInstanceGuard::acquire(&lock_file)
            .expect("first lock")
            .expect("first instance");
        assert!(
            SingleInstanceGuard::acquire(&lock_file)
                .expect("second lock")
                .is_none()
        );
        drop(first);
        assert!(
            SingleInstanceGuard::acquire(&lock_file)
                .expect("released lock")
                .is_some()
        );
    }
}
