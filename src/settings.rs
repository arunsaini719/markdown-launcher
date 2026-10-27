use std::path::PathBuf;

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, ClickEvent, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, TestSupportExt as _, Window,
    WindowBounds, WindowKind, WindowOptions,
    component::{
        Disableable as _, Theme,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
    },
    div,
    prelude::FluentBuilder as _,
    px, rgb, size,
};

use crate::config::{AppConfig, ScanConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
enum SaveStatus {
    Saved,
    Error(String),
}

pub struct SettingsView {
    config_file: PathBuf,
    defaults: AppConfig,
    global_shortcut: Entity<InputState>,
    roots: Vec<PathBuf>,
    extensions: Entity<InputState>,
    excluded_directory_names: Entity<TextareaState>,
    excluded_globs: Entity<TextareaState>,
    max_file_size_bytes: Entity<InputState>,
    follow_symbolic_links: bool,
    save_status: Option<SaveStatus>,
    picking_root: bool,
    _root_picker_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    fn new(
        config_file: PathBuf,
        config: AppConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let defaults = AppConfig::default();
        let shortcut = config.global_shortcut;
        let extensions = config.scan.extensions.join(", ");
        let excluded_directory_names = config.scan.excluded_directory_names.join("\n");
        let excluded_globs = config.scan.excluded_globs.join("\n");
        let max_file_size_bytes = config.scan.max_file_size_bytes.to_string();

        let global_shortcut = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(shortcut)
                .placeholder("cmdorctrl+shift+Space")
        });
        let extensions = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(extensions)
                .placeholder("md, markdown")
        });
        let excluded_directory_names = cx.new(|cx| {
            TextareaState::new(window, cx)
                .default_value(excluded_directory_names)
                .placeholder("One directory name per line")
        });
        let excluded_globs = cx.new(|cx| {
            TextareaState::new(window, cx)
                .default_value(excluded_globs)
                .placeholder("One root-relative glob per line")
        });
        let max_file_size_bytes = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(max_file_size_bytes)
                .placeholder("2097152")
        });
        let subscriptions = vec![
            cx.subscribe(&global_shortcut, Self::handle_input_event),
            cx.subscribe(&extensions, Self::handle_input_event),
            cx.subscribe(&excluded_directory_names, Self::handle_textarea_event),
            cx.subscribe(&excluded_globs, Self::handle_textarea_event),
            cx.subscribe(&max_file_size_bytes, Self::handle_input_event),
        ];

        Self {
            config_file,
            defaults,
            global_shortcut,
            roots: config.scan.roots,
            extensions,
            excluded_directory_names,
            excluded_globs,
            max_file_size_bytes,
            follow_symbolic_links: config.scan.follow_symbolic_links,
            save_status: None,
            picking_root: false,
            _root_picker_task: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    fn handle_input_event(
        &mut self,
        _: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) {
            self.save_status = None;
            cx.notify();
        }
    }

    fn handle_textarea_event(
        &mut self,
        _: Entity<TextareaState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) {
            self.save_status = None;
            cx.notify();
        }
    }

    fn restore_defaults(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = self.defaults.clone();
        self.roots = defaults.scan.roots.clone();
        self.follow_symbolic_links = defaults.scan.follow_symbolic_links;
        self.global_shortcut.update(cx, |input, cx| {
            input.set_value(defaults.global_shortcut.clone(), window, cx);
        });
        self.extensions.update(cx, |input, cx| {
            input.set_value(defaults.scan.extensions.join(", "), window, cx);
        });
        self.excluded_directory_names.update(cx, |input, cx| {
            input.set_value(
                defaults.scan.excluded_directory_names.join("\n"),
                window,
                cx,
            );
        });
        self.excluded_globs.update(cx, |input, cx| {
            input.set_value(defaults.scan.excluded_globs.join("\n"), window, cx);
        });
        self.max_file_size_bytes.update(cx, |input, cx| {
            input.set_value(defaults.scan.max_file_size_bytes.to_string(), window, cx);
        });
        self.save_status = None;
        cx.notify();
    }

    fn add_root(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.picking_root {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let picker = rfd::AsyncFileDialog::new()
                .set_parent(_window)
                .set_title("Choose a Markdown scan root");
            self.start_root_picker(
                async move {
                    picker
                        .pick_folder()
                        .await
                        .map(|folder| folder.path().to_path_buf())
                },
                _window,
                cx,
            );
        }

        #[cfg(not(target_os = "macos"))]
        {
            self.save_status = Some(SaveStatus::Error(
                "The folder picker is currently available only on macOS".into(),
            ));
            cx.notify();
        }
    }

    #[cfg(any(target_os = "macos", test))]
    fn start_root_picker(
        &mut self,
        selection: impl std::future::Future<Output = Option<PathBuf>> + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.picking_root {
            return;
        }
        self.picking_root = true;
        cx.notify();
        // A blocking AppKit modal loop can reenter GPUI timers while the click
        // handler holds a mutable app borrow. Open and await the sheet only
        // after this handler returns, without keeping the view/context borrowed.
        self._root_picker_task = cx.spawn_in(window, async move |this, cx| {
            let path = selection.await;
            _ = this.update_in(cx, |settings, _, cx| {
                settings.picking_root = false;
                if let Some(path) = path
                    && !settings.roots.contains(&path)
                {
                    settings.roots.push(path);
                    settings.save_status = None;
                }
                cx.notify();
            });
        });
    }

    fn remove_root(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.roots.len() {
            self.roots.remove(index);
            self.save_status = None;
            cx.notify();
        }
    }

    fn set_follow_symbolic_links(&mut self, checked: bool, cx: &mut Context<Self>) {
        self.follow_symbolic_links = checked;
        self.save_status = None;
        cx.notify();
    }

    fn save(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.save_status = Some(match self.draft_config(cx) {
            Ok(config) => match config.save(&self.config_file) {
                Ok(()) => SaveStatus::Saved,
                Err(error) => SaveStatus::Error(error.to_string()),
            },
            Err(error) => SaveStatus::Error(error),
        });
        cx.notify();
    }

    fn draft_config(&self, cx: &App) -> Result<AppConfig, String> {
        if self.roots.iter().any(|root| !root.is_absolute()) {
            return Err("Scan roots must use absolute paths".into());
        }

        let max_file_size_bytes = self
            .max_file_size_bytes
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .map_err(|_| "Maximum file size must be a positive whole number".to_string())?;
        let mut config = AppConfig {
            global_shortcut: self.global_shortcut.read(cx).value().to_string(),
            scan: ScanConfig {
                roots: self.roots.clone(),
                extensions: split_comma_or_lines(self.extensions.read(cx).value().as_ref()),
                excluded_directory_names: split_comma_or_lines(
                    self.excluded_directory_names.read(cx).value().as_ref(),
                ),
                excluded_globs: split_lines(self.excluded_globs.read(cx).value().as_ref()),
                max_file_size_bytes,
                follow_symbolic_links: self.follow_symbolic_links,
            },
        };
        config
            .normalize_and_validate()
            .map_err(|error| error.to_string())?;
        Ok(config)
    }

    fn render_field(
        label: &'static str,
        help: &'static str,
        control: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(label),
            )
            .child(div().text_xs().text_color(rgb(0x71717a)).child(help))
            .child(control)
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let default_roots = self
            .defaults
            .scan
            .roots
            .iter()
            .map(|root| root.to_string_lossy())
            .collect::<Vec<_>>()
            .join(", ");
        let default_extensions = self.defaults.scan.extensions.join(", ");
        let default_exclusions = self.defaults.scan.excluded_directory_names.join(", ");
        let default_globs = self.defaults.scan.excluded_globs.join(", ");
        let default_symlink_behavior = if self.defaults.scan.follow_symbolic_links {
            "follow"
        } else {
            "do not follow"
        };
        let default_summary = format!(
            "Shortcut: {}  •  Root: {}  •  Extensions: {}  •  Maximum size: {} bytes  •  Symlinks: {}",
            self.defaults.global_shortcut,
            default_roots,
            default_extensions,
            self.defaults.scan.max_file_size_bytes,
            default_symlink_behavior,
        );
        let root_rows = self
            .roots
            .iter()
            .enumerate()
            .map(|(index, root)| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id(format!("settings-root-{index}"))
                            .flex_grow(1.0)
                            .min_w_0()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(0x3f3f46))
                            .bg(rgb(0x27272a))
                            .px_3()
                            .py_2()
                            .text_sm()
                            .text_ellipsis()
                            .child(root.to_string_lossy().into_owned()),
                    )
                    .child(
                        Button::new(format!("remove-root-{index}"))
                            .label("Remove")
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.remove_root(index, cx);
                            })),
                    )
            })
            .collect::<Vec<_>>();
        let status = match &self.save_status {
            Some(SaveStatus::Saved) => (
                "Settings saved. Quit and reopen Markdown Launcher to apply them.".to_string(),
                rgb(0x86efac),
            ),
            Some(SaveStatus::Error(error)) => (format!("Could not save: {error}"), rgb(0xfca5a5)),
            None => (
                "Changes are validated before the configuration file is replaced.".to_string(),
                rgb(0xa1a1aa),
            ),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x18181b))
            .text_color(rgb(0xf4f4f5))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_6()
                    .pt_6()
                    .pb_4()
                    .child(div().text_2xl().font_weight(gpui_kit::FontWeight::BOLD).child("Settings"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa1a1aa))
                            .child("Saved changes take effect after the application is restarted."),
                    ),
            )
            .child(
                div()
                    .id("settings-form")
                    .flex_grow(1.0)
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .px_6()
                    .pb_6()
                    .child(
                        div()
                            .id("settings-defaults")
                            .flex()
                            .flex_col()
                            .gap_2()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(0x3f3f46))
                            .bg(rgb(0x27272a))
                            .p_4()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .child("Recommended defaults"),
                                    )
                                    .child(
                                        Button::new("settings-restore-defaults")
                                            .label("Restore Defaults")
                                            .on_click(cx.listener(Self::restore_defaults)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xa1a1aa))
                                    .child(default_summary),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x71717a))
                                    .child(format!("Excluded directory names: {default_exclusions}")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x71717a))
                                    .child(format!("Excluded path globs: {default_globs}")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xfbbf24))
                                    .child("The default root is the home directory and may trigger macOS permission prompts."),
                            ),
                    )
                    .child(Self::render_field(
                        "Global shortcut",
                        "Examples: cmdorctrl+shift+Space or alt+Space",
                        Input::new(&self.global_shortcut)
                            .bg(rgb(0xffffff))
                            .text_color(rgb(0x000000))
                            .id("settings-global-shortcut")
                            .aria_label("Global shortcut"),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                    .child("Scan roots"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x71717a))
                                                    .child("Markdown files are discovered recursively below these folders."),
                                            ),
                                    )
                                    .child(
                                        Button::new("settings-add-root")
                                            .label("Add Folder…")
                                            .disabled(self.picking_root)
                                            .on_click(cx.listener(Self::add_root)),
                                    ),
                            )
                            .child(
                                div()
                                    .id("settings-roots")
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .children(root_rows)
                                    .when(self.roots.is_empty(), |roots| {
                                        roots.child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(0xfca5a5))
                                                .child("Add at least one scan root."),
                                        )
                                    }),
                            ),
                    )
                    .child(Self::render_field(
                        "File extensions",
                        "Comma-separated extensions, with or without a leading dot.",
                        Input::new(&self.extensions)
                            .bg(rgb(0xffffff))
                            .text_color(rgb(0x000000))
                            .id("settings-extensions")
                            .aria_label("File extensions"),
                    ))
                    .child(Self::render_field(
                        "Excluded directory names",
                        "One name per line. Each matching directory is excluded at any depth.",
                        Textarea::new(&self.excluded_directory_names)
                            .bg(rgb(0xffffff))
                            .text_color(rgb(0x000000))
                            .h(px(112.0))
                            .aria_label("Excluded directory names"),
                    ))
                    .child(Self::render_field(
                        "Excluded path globs",
                        "One root-relative glob per line. Use / as the separator.",
                        Textarea::new(&self.excluded_globs)
                            .bg(rgb(0xffffff))
                            .text_color(rgb(0x000000))
                            .h(px(112.0))
                            .aria_label("Excluded path globs"),
                    ))
                    .child(Self::render_field(
                        "Maximum file size in bytes",
                        "Files above this limit are skipped and reported without stopping the scan.",
                        Input::new(&self.max_file_size_bytes)
                            .bg(rgb(0xffffff))
                            .text_color(rgb(0x000000))
                            .id("settings-max-file-size")
                            .aria_label("Maximum file size in bytes"),
                    ))
                    .child(
                        div()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(0xd4d4d8))
                            .bg(rgb(0xffffff))
                            .p_3()
                            .child(
                                Checkbox::new("settings-follow-symbolic-links")
                                    .label("Follow symbolic links")
                                    .checked(self.follow_symbolic_links)
                                    .on_click(cx.listener(|settings, checked, _, cx| {
                                        settings.set_follow_symbolic_links(*checked, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xfbbf24))
                            .child("Enable symbolic-link traversal only when crossing linked directories is intentional."),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .border_t_1()
                    .border_color(rgb(0x3f3f46))
                    .px_6()
                    .py_4()
                    .child(
                        div()
                            .id("settings-save-status")
                            .role(Role::Status)
                            .test_support()
                            .aria_label(status.0.clone())
                            .flex_grow(1.0)
                            .text_sm()
                            .text_color(status.1)
                            .child(SharedString::from(status.0)),
                    )
                    .child(
                        Button::new("settings-save")
                            .primary()
                            .label("Save Settings")
                            .on_click(cx.listener(Self::save)),
                    ),
            )
    }
}

pub fn open_settings_window(
    config_file: PathBuf,
    on_close: impl Fn(&mut App) + 'static,
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<SettingsView>), String> {
    let config = AppConfig::load_for_editing(&config_file).map_err(|error| error.to_string())?;
    let original_foreground = Theme::global(cx).foreground;
    Theme::update(cx, |theme| {
        theme.colors.foreground = rgb(0x000000).into();
    });
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(760.0), px(760.0)),
            cx,
        ))),
        window_min_size: Some(size(px(620.0), px(520.0))),
        titlebar: Some(gpui_kit::TitlebarOptions {
            title: Some("Markdown Launcher Settings".into()),
            ..Default::default()
        }),
        kind: WindowKind::Normal,
        ..Default::default()
    };

    let result = gpui_kit::open_window(options, cx, move |window, cx| {
        let settings = cx.new(|cx| SettingsView::new(config_file, config, window, cx));
        let owner = settings.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            if owner
                .upgrade()
                .is_some_and(|settings| settings.read(cx).picking_root)
            {
                return false;
            }
            Theme::update(cx, |theme| {
                theme.colors.foreground = original_foreground;
            });
            on_close(cx);
            true
        });
        settings
    });
    if result.is_err() {
        Theme::update(cx, |theme| {
            theme.colors.foreground = original_foreground;
        });
    }
    result.map_err(|error| format!("failed to open settings: {error}"))
}

fn split_comma_or_lines(value: &str) -> Vec<String> {
    value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn split_lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, test::TestWindowExt as _};

    #[test]
    fn parses_compact_and_multiline_lists() {
        assert_eq!(
            split_comma_or_lines("md, markdown\n.mdown"),
            ["md", "markdown", ".mdown"]
        );
        assert_eq!(
            split_lines("private\narchive/**\n"),
            ["private", "archive/**"]
        );
    }

    #[gpui_kit::test]
    fn saves_valid_settings_and_reports_restart_requirement(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_file = directory.path().join("config.toml");
        let root = directory.path().join("notes");
        std::fs::create_dir(&root).expect("scan root");
        let mut config = AppConfig::default_for_home(&root);
        config.global_shortcut = "alt+Space".into();
        config.save(&config_file).expect("initial configuration");

        let (window, settings) = cx
            .update(|cx| open_settings_window(config_file.clone(), |_| {}, cx))
            .expect("settings window");
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-save", cx);
            assert_eq!(
                window.find("settings-save-status").label(),
                Some("Settings saved. Quit and reopen Markdown Launcher to apply them.")
            );
        })
        .expect("settings update");

        let saved = AppConfig::load_for_editing(&config_file).expect("saved configuration");
        assert_eq!(saved.global_shortcut, "alt+Space");
        assert_eq!(saved.scan.roots, vec![root]);
        assert!(settings.read_with(cx, |settings, _| {
            settings.save_status == Some(SaveStatus::Saved)
        }));
    }

    #[gpui_kit::test]
    fn restoring_defaults_updates_the_draft_without_saving(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_file = directory.path().join("config.toml");
        let root = directory.path().join("custom-notes");
        std::fs::create_dir(&root).expect("custom root");
        let config = AppConfig::default_for_home(&root);
        config.save(&config_file).expect("initial configuration");
        let original = std::fs::read_to_string(&config_file).expect("original configuration");

        let (window, settings) = cx
            .update(|cx| open_settings_window(config_file.clone(), |_| {}, cx))
            .expect("settings window");
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-restore-defaults", cx);
        })
        .expect("restore defaults");

        assert_eq!(
            settings.read_with(cx, |settings, _| settings.roots.clone()),
            AppConfig::default().scan.roots
        );
        assert_eq!(
            std::fs::read_to_string(config_file).expect("unchanged configuration"),
            original
        );
    }

    #[gpui_kit::test]
    fn replacing_the_last_root_yields_to_app_updates_before_selection(cx: &mut TestAppContext) {
        use std::{cell::Cell, rc::Rc, time::Duration};
        let (directory, config_file, window, settings) = open_picker_test_settings(cx);
        let original = std::fs::read(&config_file).unwrap();
        let replacement = directory.path().join("replacement");
        std::fs::create_dir(&replacement).unwrap();
        let selected = replacement.clone();
        let app = cx.to_async();
        let ticks = Rc::new(Cell::new(0));
        let progress = Rc::clone(&ticks);
        cx.update_window(window, |_, window, cx| {
            settings.update(cx, |settings, cx| {
                settings.remove_root(0, cx);
                let timer = cx.background_executor().timer(Duration::from_millis(500));
                settings.start_root_picker(
                    async move {
                        app.update(|_| progress.set(1));
                        timer.await;
                        app.update(|_| progress.set(2));
                        Some(selected)
                    },
                    window,
                    cx,
                );
                assert!(settings.roots.is_empty());
                assert!(settings.picking_root);
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(ticks.get(), 1);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            settings.update(cx, |settings, cx| {
                settings.set_follow_symbolic_links(true, cx)
            });
        })
        .unwrap();
        assert_eq!(std::fs::read(&config_file).unwrap(), original);
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        assert_eq!(ticks.get(), 2);
        settings.read_with(cx, |settings, _| {
            assert_eq!(settings.roots, vec![replacement.clone()]);
            assert!(!settings.picking_root);
            assert!(settings.follow_symbolic_links);
        });
        assert_eq!(std::fs::read(&config_file).unwrap(), original);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-save", cx);
        })
        .unwrap();
        let saved = AppConfig::load_for_editing(&config_file).unwrap();
        assert_eq!(saved.scan.roots, vec![replacement]);
    }

    #[gpui_kit::test]
    fn cancelling_a_picker_after_removing_the_last_root_does_not_broaden_or_save(
        cx: &mut TestAppContext,
    ) {
        let (_directory, config_file, window, settings) = open_picker_test_settings(cx);
        let original = std::fs::read(&config_file).unwrap();
        cx.update_window(window, |_, window, cx| {
            settings.update(cx, |settings, cx| {
                settings.remove_root(0, cx);
                settings.start_root_picker(async { None }, window, cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
        settings.read_with(cx, |settings, _| {
            assert!(settings.roots.is_empty());
            assert!(!settings.picking_root);
        });
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-save", cx);
        })
        .unwrap();
        assert!(settings.read_with(cx, |settings, _| matches!(&settings.save_status, Some(SaveStatus::Error(error)) if error.contains("scan root"))));
        assert_eq!(std::fs::read(&config_file).unwrap(), original);
    }

    #[gpui_kit::test]
    fn a_pending_picker_cannot_be_replaced_and_duplicate_roots_are_ignored(
        cx: &mut TestAppContext,
    ) {
        use std::time::Duration;
        let (directory, config_file, window, settings) = open_picker_test_settings(cx);
        let original = std::fs::read(&config_file).unwrap();
        let root = directory.path().join("notes");
        let duplicate = root.clone();
        let ignored = directory.path().join("ignored");
        cx.update_window(window, |_, window, cx| {
            settings.update(cx, |settings, cx| {
                settings.save_status = Some(SaveStatus::Saved);
                let timer = cx.background_executor().timer(Duration::from_millis(500));
                settings.start_root_picker(
                    async move {
                        timer.await;
                        Some(duplicate)
                    },
                    window,
                    cx,
                );
                settings.start_root_picker(async move { Some(ignored) }, window, cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
        assert!(settings.read_with(cx, |settings, _| settings.picking_root));
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        settings.read_with(cx, |settings, _| {
            assert_eq!(settings.roots, vec![root]);
            assert!(!settings.picking_root);
            assert_eq!(settings.save_status, Some(SaveStatus::Saved));
        });
        assert_eq!(std::fs::read(&config_file).unwrap(), original);
    }

    #[gpui_kit::test]
    fn the_settings_parent_stays_open_until_its_folder_picker_finishes(cx: &mut TestAppContext) {
        use std::time::Duration;
        let (_directory, _, window, settings) = open_picker_test_settings(cx);
        cx.update_window(window, |_, window, cx| {
            settings.update(cx, |settings, cx| {
                let timer = cx.background_executor().timer(Duration::from_millis(500));
                settings.start_root_picker(
                    async move {
                        timer.await;
                        None
                    },
                    window,
                    cx,
                );
            })
        })
        .unwrap();
        cx.run_until_parked();
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(!visual.simulate_close());
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        assert!(visual.simulate_close());
    }

    fn open_picker_test_settings(
        cx: &mut TestAppContext,
    ) -> (
        tempfile::TempDir,
        PathBuf,
        AnyWindowHandle,
        Entity<SettingsView>,
    ) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let config_file = directory.path().join("config.toml");
        AppConfig::default_for_home(&root)
            .save(&config_file)
            .unwrap();
        let (window, settings) = cx
            .update(|cx| open_settings_window(config_file.clone(), |_| {}, cx))
            .unwrap();
        (directory, config_file, window, settings)
    }

    #[gpui_kit::test]
    fn rejects_saving_without_a_scan_root(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_file = directory.path().join("config.toml");
        AppConfig::default_for_home(directory.path())
            .save(&config_file)
            .expect("initial configuration");
        let original = std::fs::read_to_string(&config_file).expect("original configuration");

        let (window, settings) = cx
            .update(|cx| open_settings_window(config_file.clone(), |_| {}, cx))
            .expect("settings window");
        settings.update(cx, |settings, cx| {
            settings.roots.clear();
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-save", cx);
        })
        .expect("settings update");

        assert!(settings.read_with(cx, |settings, _| {
            matches!(
                &settings.save_status,
                Some(SaveStatus::Error(error)) if error.contains("scan root")
            )
        }));
        assert_eq!(
            std::fs::read_to_string(config_file).expect("unchanged configuration"),
            original
        );
    }
}
