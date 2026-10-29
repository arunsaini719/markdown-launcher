use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use directories::BaseDirs;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    Disableable as _, Sizable as _,
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, KeyBinding, ListAlignment, ListState, ParentElement as _,
    Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, TestSupportExt as _, UniformListScrollHandle, Window, div, list, px, rgb,
    uniform_list,
};

use crate::{
    config::ScanConfig,
    desktop::{DesktopMenu, DesktopMenuAction, LaunchAtLoginChange, SingleInstanceGuard},
    editor::{EDITOR_KEY_CONTEXT, EditorEvent, EditorWorkspace},
    index::{
        IndexActivity, IndexDatabase, IndexWatchService, SearchResult, WatchServiceStatus,
        read_markdown_body,
    },
    new_note::{NewNoteEvent, NewNoteView},
    notes::{CreatedNote, load_last_destination},
    preview::{BlockKind, PreviewDocument},
    preview_selection::PreviewSelection,
    settings::open_settings_window,
    workspace_ui::{action_button, document_header, primary_button},
};

const KEY_CONTEXT: &str = "MarkdownLauncher";
const PREVIEW_KEY_CONTEXT: &str = "MarkdownPreview";
const RESULT_LIMIT: usize = 50;
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(500);
const HOTKEY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const DESKTOP_MENU_POLL_INTERVAL: Duration = Duration::from_millis(50);
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(100);
const CARET_COLOR: u32 = 0x3b82f6;

#[cfg(test)]
mod workspace_tests;

#[derive(Debug, Default, PartialEq, Eq)]
enum PreviewState {
    #[default]
    Empty,
    Loading(PathBuf),
    Ready {
        path: PathBuf,
        document: PreviewDocument,
    },
    Failed {
        path: PathBuf,
        error: String,
    },
}

gpui_kit::actions!(
    markdown_launcher,
    [
        SelectPrevious,
        SelectNext,
        SelectFirst,
        SelectLast,
        ActivateSelected,
        FocusPreview,
        PreviewPageUp,
        PreviewPageDown,
        PreviewStart,
        PreviewEnd,
        CopyPreviewSelection,
        SelectAllPreview,
        Dismiss,
        RebuildIndex,
        OpenSettings,
        NewNote,
        EditSelected,
        SaveEditor,
        QuitApplication
    ]
);

pub fn init_launcher(cx: &mut App) {
    // Input carets use the theme, not local text_color. This blue contrasts
    // with both the dark workspace and the white settings/dialog inputs.
    gpui_kit::component::Theme::update(cx, |theme| theme.colors.caret = rgb(CARET_COLOR).into());
    crate::new_note::init(cx);
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-home", SelectFirst, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-end", SelectLast, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", FocusPreview, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-c", ActivateSelected, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-c", ActivateSelected, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-c", CopyPreviewSelection, Some(PREVIEW_KEY_CONTEXT)),
        KeyBinding::new("ctrl-c", CopyPreviewSelection, Some(PREVIEW_KEY_CONTEXT)),
        KeyBinding::new("cmd-a", SelectAllPreview, Some(PREVIEW_KEY_CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAllPreview, Some(PREVIEW_KEY_CONTEXT)),
        KeyBinding::new("pageup", PreviewPageUp, Some(PREVIEW_KEY_CONTEXT)),
        KeyBinding::new("pagedown", PreviewPageDown, Some(PREVIEW_KEY_CONTEXT)),
        KeyBinding::new("home", PreviewStart, Some(PREVIEW_KEY_CONTEXT)),
        KeyBinding::new("end", PreviewEnd, Some(PREVIEW_KEY_CONTEXT)),
        KeyBinding::new("cmd-up", PreviewStart, Some(PREVIEW_KEY_CONTEXT)),
        KeyBinding::new("cmd-down", PreviewEnd, Some(PREVIEW_KEY_CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-r", RebuildIndex, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-r", RebuildIndex, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-,", OpenSettings, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-q", QuitApplication, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-n", NewNote, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-n", NewNote, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-e", EditSelected, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-e", EditSelected, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-s", SaveEditor, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-s", SaveEditor, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(EDITOR_KEY_CONTEXT)),
        KeyBinding::new("cmd-s", SaveEditor, Some(EDITOR_KEY_CONTEXT)),
        KeyBinding::new("ctrl-s", SaveEditor, Some(EDITOR_KEY_CONTEXT)),
        KeyBinding::new("cmd-e", EditSelected, Some(EDITOR_KEY_CONTEXT)),
        KeyBinding::new("cmd-q", QuitApplication, Some(EDITOR_KEY_CONTEXT)),
        KeyBinding::new("cmd-,", OpenSettings, Some(EDITOR_KEY_CONTEXT)),
        KeyBinding::new("cmd-n", NewNote, Some(EDITOR_KEY_CONTEXT)),
        KeyBinding::new("ctrl-n", NewNote, Some(EDITOR_KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-c", ActivateSelected, Some(EDITOR_KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-c", ActivateSelected, Some(EDITOR_KEY_CONTEXT)),
    ]);
}

pub struct RegisteredGlobalShortcut {
    _manager: GlobalHotKeyManager,
    hotkey_id: u32,
}

impl RegisteredGlobalShortcut {
    pub fn register(shortcut: &str) -> Result<Self, String> {
        let hotkey = shortcut
            .parse::<HotKey>()
            .map_err(|error| error.to_string())?;
        let manager = GlobalHotKeyManager::new().map_err(|error| error.to_string())?;
        manager
            .register(hotkey)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            _manager: manager,
            hotkey_id: hotkey.id(),
        })
    }
}

#[derive(Default)]
pub struct LauncherServices {
    pub watch_service: Option<IndexWatchService>,
    pub global_shortcut: Option<RegisteredGlobalShortcut>,
    pub desktop_menu: Option<DesktopMenu>,
    pub single_instance: Option<SingleInstanceGuard>,
    pub config_file: Option<PathBuf>,
    pub scan_config: Option<ScanConfig>,
}

pub struct Launcher {
    query: Entity<InputState>,
    editor: Entity<EditorWorkspace>,
    _editor_subscription: Subscription,
    quitting: bool,
    focus: FocusHandle,
    preview_focus: FocusHandle,
    results: Vec<SearchResult>,
    selected_index: Option<usize>,
    preview: PreviewState,
    preview_generation: Arc<AtomicU64>,
    preview_scroll: ListState,
    preview_selection: PreviewSelection,
    preview_layouts: Rc<RefCell<BTreeMap<usize, gpui_kit::TextLayout>>>,
    preview_drag_point: Option<gpui_kit::Point<gpui_kit::Pixels>>,
    _selection_scroll_task: Task<()>,
    scroll_handle: UniformListScrollHandle,
    database_path: PathBuf,
    home_directory: Option<PathBuf>,
    max_file_size_bytes: u64,
    searching: bool,
    search_query: String,
    results_query: Option<String>,
    search_error: Option<String>,
    search_generation: u64,
    copy_generation: u64,
    copying: bool,
    seen_index_revision: u64,
    index_status: Option<WatchServiceStatus>,
    startup_error: Option<String>,
    action_status: Option<String>,
    settings_window: Option<AnyWindowHandle>,
    settings_open: bool,
    config_file: Option<PathBuf>,
    scan_config: Option<ScanConfig>,
    creation: Option<Entity<NewNoteView>>,
    last_note_destination: Option<PathBuf>,
    created_result: Option<SearchResult>,
    creation_warning: Option<String>,
    creation_return_to_preview: bool,
    creation_return_to_editor: bool,
    _creation_subscription: Option<Subscription>,
    _watch_service: Option<IndexWatchService>,
    _global_shortcut: Option<RegisteredGlobalShortcut>,
    _desktop_menu: Option<DesktopMenu>,
    _single_instance: Option<SingleInstanceGuard>,
    _search_task: Task<()>,
    _copy_task: Task<()>,
    _preview_task: Task<()>,
    _status_task: Task<()>,
    _hotkey_task: Task<()>,
    _desktop_menu_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Launcher {
    pub fn new(
        database_path: PathBuf,
        max_file_size_bytes: u64,
        services: LauncherServices,
        startup_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search Markdown files"));
        let query_subscription = cx.subscribe_in(&query, window, Self::handle_query_input_event);
        let activation_subscription =
            cx.observe_window_activation(window, |launcher, window, cx| {
                if !window.is_window_active()
                    && !launcher.settings_open
                    && launcher.creation.is_none()
                {
                    launcher.finish_edit(cx);
                    launcher.cancel_copy();
                    hide_application(cx);
                }
            });
        let launcher = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            let busy = launcher
                .update(cx, |launcher, cx| {
                    launcher.finish_edit(cx);
                    launcher.cancel_copy();
                    launcher
                        .creation
                        .as_ref()
                        .is_some_and(|view| view.read(cx).is_busy())
                })
                .unwrap_or(false);
            if !busy {
                hide_application(cx);
            }
            false
        });
        let editor = cx.new(|_| EditorWorkspace::new());
        let editor_subscription =
            cx.subscribe_in(&editor, window, |launcher, editor, event, window, cx| {
                match event {
                    EditorEvent::Saved(path) => {
                        if let Some(service) = &launcher._watch_service {
                            service.request_sync(path.clone());
                        }
                        if editor.read(cx).path() != Some(path.as_path())
                            && launcher
                                .selected_index
                                .and_then(|index| launcher.results.get(index))
                                .is_some_and(|result| result.canonical_path == *path)
                        {
                            launcher.load_selected_preview(cx);
                        }
                    }
                    EditorEvent::QuitReady => {
                        cx.quit();
                    }
                    EditorEvent::Changed => {
                        editor.update(cx, |editor, cx| editor.resume_open(window, cx));
                    }
                }
                cx.notify();
            });
        let quit_subscription = cx.on_app_quit(|launcher, cx| {
            launcher
                .editor
                .update(cx, |editor, cx| editor.prepare_shutdown(cx))
        });
        // Refreshes can remove or disable a keyboard-focused row control.
        let focus_lost_subscription = cx.on_focus_lost(window, |launcher, window, cx| {
            if launcher.quitting || launcher.settings_open {
                return;
            }
            if let Some(dialog) = &launcher.creation {
                if !dialog.read(cx).is_busy() {
                    dialog.update(cx, |dialog, cx| dialog.focus(window, cx));
                }
            } else if launcher.editor.read(cx).has_draft() {
                launcher
                    .editor
                    .update(cx, |editor, cx| editor.focus(window, cx));
            } else {
                launcher
                    .query
                    .update(cx, |input, cx| input.focus(window, cx));
            }
        });
        let focus = cx.focus_handle();
        query.update(cx, |input, cx| input.focus(window, cx));

        let mut launcher = Self {
            query,
            editor,
            _editor_subscription: editor_subscription,
            quitting: false,
            focus,
            preview_focus: cx.focus_handle(),
            results: Vec::new(),
            selected_index: None,
            preview: PreviewState::Empty,
            preview_generation: Arc::new(AtomicU64::new(0)),
            preview_scroll: ListState::new(0, ListAlignment::Top, px(300.0)),
            preview_selection: PreviewSelection::default(),
            preview_layouts: Rc::default(),
            preview_drag_point: None,
            _selection_scroll_task: Task::ready(()),
            scroll_handle: UniformListScrollHandle::new(),
            database_path,
            home_directory: BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()),
            max_file_size_bytes,
            searching: false,
            search_query: String::new(),
            results_query: None,
            search_error: None,
            search_generation: 0,
            copy_generation: 0,
            copying: false,
            seen_index_revision: 0,
            index_status: None,
            startup_error,
            action_status: None,
            settings_window: None,
            settings_open: false,
            config_file: services.config_file,
            scan_config: services.scan_config,
            creation: None,
            last_note_destination: None,
            created_result: None,
            creation_warning: None,
            creation_return_to_preview: false,
            creation_return_to_editor: false,
            _creation_subscription: None,
            _watch_service: services.watch_service,
            _global_shortcut: services.global_shortcut,
            _desktop_menu: services.desktop_menu,
            _single_instance: services.single_instance,
            _search_task: Task::ready(()),
            _copy_task: Task::ready(()),
            _preview_task: Task::ready(()),
            _status_task: Task::ready(()),
            _hotkey_task: Task::ready(()),
            _desktop_menu_task: Task::ready(()),
            _subscriptions: vec![
                query_subscription,
                activation_subscription,
                quit_subscription,
                focus_lost_subscription,
            ],
        };
        launcher.start_search(String::new(), cx);
        launcher.start_status_poll(cx);
        launcher.start_hotkey_poll(window, cx);
        launcher.start_desktop_menu_poll(window, cx);
        launcher
    }

    fn handle_query_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) {
            self.start_search(input.read(cx).value().to_string(), cx);
        }
    }

    fn start_search(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        let database_path = self.database_path.clone();
        self.searching = true;
        if self.search_query != query {
            self.created_result = None;
            self.creation_warning = None;
        }
        if self.search_query != query {
            self.finish_edit(cx);
        }
        let editing_path = self.editor.read(cx).path().map(Path::to_path_buf);
        let created_result = editing_path
            .as_deref()
            .and_then(|path| {
                self.results
                    .iter()
                    .find(|result| result.canonical_path == path)
            })
            .cloned()
            .or_else(|| self.created_result.clone());
        self.search_query = query.clone();
        if self.results_query.as_deref() != Some(query.as_str()) {
            self.selected_index = None;
            self.clear_preview();
            self.cancel_copy();
        }
        self.search_error = None;
        self.action_status = None;
        cx.notify();

        self._search_task = cx.spawn(async move |this, cx| {
            let search_query = query.clone();
            let result = cx
                .background_spawn(async move {
                    let mut results = IndexDatabase::open(&database_path)
                        .and_then(|database| database.search(&search_query, RESULT_LIMIT))
                        .map_err(|error| error.to_string())?;
                    if let Some(created) = created_result
                        && (editing_path.as_deref() == Some(created.canonical_path.as_path())
                            || std::fs::symlink_metadata(&created.canonical_path)
                                .is_ok_and(|metadata| metadata.is_file()))
                        && !results
                            .iter()
                            .any(|result| result.canonical_path == created.canonical_path)
                    {
                        results.insert(0, created);
                        results.truncate(RESULT_LIMIT);
                    }
                    Ok::<_, String>(results)
                })
                .await;

            _ = this.update(cx, |launcher, cx| {
                if !is_current_search_generation(launcher.search_generation, generation) {
                    return;
                }
                launcher.searching = false;
                match result {
                    Ok(results) => {
                        if launcher.created_result.as_ref().is_some_and(|created| {
                            !results
                                .iter()
                                .any(|result| result.canonical_path == created.canonical_path)
                        }) {
                            launcher.created_result = None;
                        }
                        let preserve = launcher.results_query.as_deref() == Some(query.as_str());
                        let selected = selection_after_refresh(
                            launcher.selected_index,
                            &launcher.results,
                            &results,
                            preserve,
                        );
                        let selection_changed = selected != launcher.selected_index;
                        launcher.results = results;
                        launcher.results_query = Some(query);
                        launcher.selected_index = selected;
                        // A same-query index refresh may represent an external edit.
                        // Re-read only an explicitly selected document.
                        launcher.load_selected_preview(cx);
                        if let Some(index) = selected
                            && (selection_changed || !preserve)
                        {
                            launcher.scroll_handle.scroll_to_item(
                                index,
                                if preserve {
                                    ScrollStrategy::Center
                                } else {
                                    ScrollStrategy::Top
                                },
                            );
                        }
                    }
                    Err(error) => {
                        let preserve_created =
                            launcher.created_result.as_ref().is_some_and(|created| {
                                launcher
                                    .selected_index
                                    .and_then(|index| launcher.results.get(index))
                                    .is_some_and(|result| {
                                        result.canonical_path == created.canonical_path
                                    })
                            });
                        launcher.results = launcher.created_result.clone().into_iter().collect();
                        launcher.results_query = Some(query);
                        launcher.selected_index = preserve_created.then_some(0);
                        if !preserve_created {
                            launcher.clear_preview();
                        }
                        launcher.search_error = Some(error);
                    }
                }
                cx.notify();
            });
        });
    }

    fn start_status_poll(&mut self, cx: &mut Context<Self>) {
        self._status_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(STATUS_POLL_INTERVAL).await;
                let keep_polling = this
                    .update(cx, |launcher, cx| {
                        let Some(status) = launcher
                            ._watch_service
                            .as_ref()
                            .map(IndexWatchService::status)
                        else {
                            return false;
                        };

                        if launcher.seen_index_revision != status.revision {
                            launcher.seen_index_revision = status.revision;
                            let query = launcher.query.read(cx).value().to_string();
                            launcher.start_search(query, cx);
                        }
                        launcher.index_status = Some(status);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_polling {
                    break;
                }
            }
        });
    }

    fn start_hotkey_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hotkey_id) = self
            ._global_shortcut
            .as_ref()
            .map(|shortcut| shortcut.hotkey_id)
        else {
            return;
        };

        self._hotkey_task = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(HOTKEY_POLL_INTERVAL).await;
                let pressed = GlobalHotKeyEvent::receiver()
                    .try_iter()
                    .any(|event| event.id == hotkey_id && event.state == HotKeyState::Pressed);
                if pressed
                    && this
                        .update_in(cx, |launcher, window, cx| {
                            launcher.show_launcher(window, cx);
                        })
                        .is_err()
                {
                    break;
                }
            }
        });
    }

    fn start_desktop_menu_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._desktop_menu.is_none() {
            return;
        }

        self._desktop_menu_task = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(DESKTOP_MENU_POLL_INTERVAL)
                    .await;
                let keep_polling = this
                    .update_in(cx, |launcher, window, cx| {
                        let action = launcher._desktop_menu.as_mut().and_then(|menu| {
                            menu.refresh_launch_at_login_status();
                            menu.next_action()
                        });
                        match action {
                            Some(DesktopMenuAction::OpenLauncher) => {
                                launcher.show_launcher(window, cx);
                                true
                            }
                            Some(DesktopMenuAction::OpenSettings) => {
                                launcher.show_settings(window, cx);
                                true
                            }
                            Some(DesktopMenuAction::RebuildIndex) => {
                                launcher.request_rebuild(cx);
                                true
                            }
                            Some(DesktopMenuAction::ToggleLaunchAtLogin) => {
                                launcher.toggle_launch_at_login(window, cx);
                                true
                            }
                            Some(DesktopMenuAction::Quit) => {
                                launcher.quit_application(&QuitApplication, window, cx);
                                true
                            }
                            None => true,
                        }
                    })
                    .unwrap_or(false);
                if !keep_polling {
                    break;
                }
            }
        });
    }

    fn show_settings(&mut self, launcher_window: &mut Window, cx: &mut Context<Self>) {
        if self.creation.is_some() {
            self.show_launcher(launcher_window, cx);
            return;
        }
        self.finish_edit(cx);
        self.cancel_copy();
        if let Some(handle) = self.settings_window {
            self.settings_open = true;
            if let Err(error) = hide_launcher_for_settings(launcher_window) {
                self.settings_open = false;
                self.action_status = Some(format!("Could not hide the launcher: {error}"));
                cx.notify();
                return;
            }
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                cx.activate(true);
                return;
            }
            self.settings_open = false;
            self.settings_window = None;
        }

        let Some(config_file) = self.config_file.clone() else {
            self.show_launcher(launcher_window, cx);
            self.action_status = Some("The settings configuration path is unavailable".into());
            cx.notify();
            return;
        };
        self.settings_open = true;
        if let Err(error) = hide_launcher_for_settings(launcher_window) {
            self.settings_open = false;
            self.action_status = Some(format!("Could not hide the launcher: {error}"));
            cx.notify();
            return;
        }
        let owner = cx.entity().downgrade();
        let result = open_settings_window(
            config_file,
            move |cx| {
                _ = owner.update(cx, |launcher, cx| {
                    launcher.settings_open = false;
                    launcher.settings_window = None;
                    cx.notify();
                });
            },
            cx,
        );
        match result {
            Ok((handle, _)) => {
                self.settings_window = Some(handle);
                cx.activate(true);
            }
            Err(error) => {
                self.settings_open = false;
                self.settings_window = None;
                self.show_launcher(launcher_window, cx);
                self.action_status = Some(format!("Could not open settings: {error}"));
                cx.notify();
            }
        }
    }

    fn toggle_launch_at_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = self
            ._desktop_menu
            .as_mut()
            .ok_or_else(|| "the macOS menu bar is unavailable".to_string())
            .and_then(DesktopMenu::toggle_launch_at_login);
        if result.is_err() {
            self.show_launcher(window, cx);
        }
        self.action_status = Some(match result {
            Ok(LaunchAtLoginChange::Enabled) => "Launch at Login enabled".into(),
            Ok(LaunchAtLoginChange::Disabled) => "Launch at Login disabled".into(),
            Ok(LaunchAtLoginChange::ApprovalRequired) => {
                "Approve Launch at Login in System Settings".into()
            }
            Err(error) => error,
        });
        cx.notify();
    }

    fn show_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.activate(true);
        window.activate_window();
        self.action_status = None;
        if let Some(view) = &self.creation {
            view.update(cx, |view, cx| view.focus(window, cx));
        } else {
            self.query.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn new_note(&mut self, _: &NewNote, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            self.show_settings(window, cx);
            return;
        }
        if self.creation.is_some() {
            return;
        }
        let Some(config) = self.scan_config.clone() else {
            self.action_status = Some(
                "Note creation is unavailable because the active scan settings are unavailable"
                    .into(),
            );
            cx.notify();
            return;
        };
        if !self.editor.read(cx).has_draft() {
            self.finish_edit(cx);
        }
        self.cancel_copy();
        let preference_file = self
            .config_file
            .as_ref()
            .map(|path| path.with_file_name("note-creation.json"));
        let remembered = self
            .last_note_destination
            .clone()
            .or_else(|| preference_file.as_deref().and_then(load_last_destination));
        self.creation_return_to_preview = self.preview_focus.is_focused(window);
        self.creation_return_to_editor = self.editor.read(cx).is_focused(window, cx);
        let query = self.query.read(cx).value().to_string();
        let view = cx.new(|cx| {
            NewNoteView::new(
                config,
                &query,
                remembered.as_deref(),
                preference_file,
                window,
                cx,
            )
        });
        self._creation_subscription =
            Some(
                cx.subscribe_in(&view, window, |launcher, view, event, window, cx| {
                    view.update(cx, |view, cx| view.restore_theme(cx));
                    launcher.creation = None;
                    launcher._creation_subscription = None;
                    match event {
                        NewNoteEvent::Cancelled => {
                            if launcher.creation_return_to_editor
                                && launcher.editor.read(cx).has_draft()
                            {
                                launcher
                                    .editor
                                    .update(cx, |editor, cx| editor.focus(window, cx));
                            } else if launcher.creation_return_to_preview
                                && launcher.selected_preview_ready()
                            {
                                launcher.preview_focus.focus(window, cx);
                            } else {
                                launcher
                                    .query
                                    .update(cx, |input, cx| input.focus(window, cx));
                            }
                        }
                        NewNoteEvent::Created(note) => {
                            launcher.complete_note_creation(note, window, cx)
                        }
                    }
                    cx.notify();
                }),
            );
        self.creation = Some(view);
        cx.notify();
    }

    fn complete_note_creation(
        &mut self,
        note: &CreatedNote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.last_note_destination = note.path.parent().map(Path::to_path_buf);
        self.creation_warning = note.warning.clone();
        let result = SearchResult {
            canonical_path: note.path.clone(),
            title: note.title.clone(),
            tags: Vec::new(),
            excerpt: note.body.clone(),
            score: 0.0,
        };
        self.search_generation = self.search_generation.wrapping_add(1);
        self._search_task = Task::ready(());
        self.searching = false;
        self.search_query.clear();
        self.results_query = Some(String::new());
        self.created_result = Some(result.clone());
        self.results = vec![result];
        self.selected_index = Some(0);
        self.search_error = None;
        self.query.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.load_selected_preview(cx);
        self.scroll_to_selection();
        if let Some(service) = &self._watch_service {
            service.request_sync(note.path.clone());
        }
        if let Some(config) = self.scan_config.clone() {
            self.editor.update(cx, |editor, cx| {
                editor.open(config, note.path.clone(), window, cx)
            });
        }
    }

    fn edit_selected(&mut self, _: &EditSelected, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || self.creation.is_some() || self.quitting {
            return;
        }
        let Some(config) = self.scan_config.clone() else {
            return;
        };
        let Some(path) = self
            .selected_index
            .and_then(|index| self.results.get(index))
            .map(|result| result.canonical_path.clone())
        else {
            return;
        };
        self.cancel_copy();
        self.editor
            .update(cx, |editor, cx| editor.open(config, path, window, cx));
        cx.notify();
    }

    fn save_editor(&mut self, _: &SaveEditor, _: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| editor.request_save(cx));
    }

    fn finish_edit(&mut self, cx: &mut Context<Self>) {
        if self.editor.read(cx).path().is_some() {
            self.editor.update(cx, |editor, cx| editor.leave(cx));
            self.load_selected_preview(cx);
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.read(cx).is_focused(window, cx) {
            return;
        }
        if self.preview_focus.is_focused(window) {
            self.preview_scroll.scroll_by(px(-40.0));
            cx.notify();
            return;
        }
        if self.results_query.as_deref() != Some(self.search_query.as_str()) {
            return;
        }
        let selected = previous_selection(self.selected_index, self.results.len());
        if selected != self.selected_index {
            self.selected_index = selected;
            self.load_selected_preview(cx);
        }
        self.query.update(cx, |input, cx| input.focus(window, cx));
        self.scroll_to_selection();
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.read(cx).is_focused(window, cx) {
            return;
        }
        if self.preview_focus.is_focused(window) {
            self.preview_scroll.scroll_by(px(40.0));
            cx.notify();
            return;
        }
        if self.results_query.as_deref() != Some(self.search_query.as_str()) {
            return;
        }
        let selected = next_selection(self.selected_index, self.results.len());
        if selected != self.selected_index {
            self.selected_index = selected;
            self.load_selected_preview(cx);
        }
        self.query.update(cx, |input, cx| input.focus(window, cx));
        self.scroll_to_selection();
        cx.notify();
    }

    fn select_first(&mut self, _: &SelectFirst, window: &mut Window, cx: &mut Context<Self>) {
        self.select_boundary(false, window, cx);
    }

    fn select_last(&mut self, _: &SelectLast, window: &mut Window, cx: &mut Context<Self>) {
        self.select_boundary(true, window, cx);
    }

    fn select_boundary(&mut self, last: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.read(cx).is_focused(window, cx)
            || self.preview_focus.is_focused(window)
            || self.results_query.as_deref() != Some(self.search_query.as_str())
        {
            return;
        }
        let selected =
            (!self.results.is_empty()).then(|| if last { self.results.len() - 1 } else { 0 });
        if selected != self.selected_index {
            self.selected_index = selected;
            self.load_selected_preview(cx);
        }
        self.query.update(cx, |input, cx| input.focus(window, cx));
        self.scroll_to_selection();
        cx.notify();
    }

    fn clear_preview(&mut self) {
        self.preview_generation.fetch_add(1, Ordering::Relaxed);
        self._preview_task = Task::ready(());
        self.preview = PreviewState::Empty;
        self.preview_scroll.reset(0);
        self.preview_selection = PreviewSelection::default();
        self.preview_drag_point = None;
        self.preview_layouts.borrow_mut().clear();
        self._selection_scroll_task = Task::ready(());
    }

    fn load_selected_preview(&mut self, cx: &mut Context<Self>) {
        let path = self
            .selected_index
            .and_then(|index| self.results.get(index))
            .map(|result| result.canonical_path.as_path());
        if let Some(editing_path) = self.editor.read(cx).path() {
            if Some(editing_path) == path {
                return;
            }
            self.editor.update(cx, |editor, cx| editor.leave(cx));
        }
        self.clear_preview();
        self.cancel_copy();
        let Some(path) = self
            .selected_index
            .and_then(|index| self.results.get(index))
            .map(|result| result.canonical_path.clone())
        else {
            return;
        };
        let generation = self.preview_generation.load(Ordering::Relaxed);
        let token = Arc::clone(&self.preview_generation);
        let maximum = self.max_file_size_bytes;
        self.preview = PreviewState::Loading(path.clone());
        self._preview_task = cx.spawn(async move |this, cx| {
            // Dropping this task replaces queued/debounced work on rapid navigation.
            // An OS read already in progress may finish, but cannot publish stale data.
            cx.background_executor().timer(PREVIEW_DEBOUNCE).await;
            let read_path = path.clone();
            let result = cx
                .background_spawn(async move {
                    if token.load(Ordering::Relaxed) != generation {
                        return None;
                    }
                    let result = PreviewDocument::load(&read_path, maximum);
                    (token.load(Ordering::Relaxed) == generation).then_some(result)
                })
                .await;
            if let Some(result) = result {
                _ = this.update(cx, |launcher, cx| {
                    launcher.complete_preview(generation, path, result, cx);
                });
            }
        });
    }

    fn complete_preview(
        &mut self,
        generation: u64,
        path: PathBuf,
        result: Result<PreviewDocument, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.preview_generation.load(Ordering::Relaxed)
            || self
                .selected_index
                .and_then(|index| self.results.get(index))
                .is_none_or(|selected| selected.canonical_path != path)
        {
            return;
        }
        self.preview = match result {
            Ok(document) => {
                self.preview_scroll.reset(document.blocks.len());
                PreviewState::Ready { path, document }
            }
            Err(error) => PreviewState::Failed { path, error },
        };
        cx.notify();
    }

    fn selected_preview_ready(&self) -> bool {
        !self.searching
            && self.results_query.as_deref() == Some(self.search_query.as_str())
            && matches!(&self.preview, PreviewState::Ready { path, .. }
                if self.selected_index.and_then(|index| self.results.get(index))
                    .is_some_and(|selected| selected.canonical_path == *path))
    }

    fn focus_preview(&mut self, _: &FocusPreview, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.read(cx).path().is_some() {
            self.editor
                .update(cx, |editor, cx| editor.focus(window, cx));
            return;
        }
        if self.selected_preview_ready() {
            self.preview_focus.focus(window, cx);
            cx.notify();
        }
    }

    fn preview_page_up(&mut self, _: &PreviewPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.preview_scroll
            .scroll_by(-self.preview_scroll.viewport_bounds().size.height);
        cx.notify();
    }

    fn preview_page_down(&mut self, _: &PreviewPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.preview_scroll
            .scroll_by(self.preview_scroll.viewport_bounds().size.height);
        cx.notify();
    }

    fn preview_start(&mut self, _: &PreviewStart, _: &mut Window, cx: &mut Context<Self>) {
        self.preview_scroll.scroll_to(gpui_kit::ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        cx.notify();
    }

    fn preview_end(&mut self, _: &PreviewEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.preview_scroll.scroll_to_end();
        cx.notify();
    }

    fn activate_selected(
        &mut self,
        _: &ActivateSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.selected_index else {
            return;
        };
        if self.row_copy_unavailable(index, cx).is_some() {
            return;
        }
        let Some(path) = self
            .selected_index
            .and_then(|index| self.results.get(index))
            .map(|result| result.canonical_path.clone())
        else {
            return;
        };
        self.copy_generation = self.copy_generation.wrapping_add(1);
        let generation = self.copy_generation;
        let max_file_size_bytes = self.max_file_size_bytes;
        self.copying = true;
        self.action_status = Some("Copying current Markdown body...".into());
        cx.notify();

        self._copy_task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    read_markdown_body(&path, max_file_size_bytes)
                        .map_err(|error| error.to_string())
                })
                .await;

            _ = this.update_in(cx, |launcher, window, cx| {
                launcher.complete_copy(generation, result, window, cx);
            });
        });
    }

    fn activate_clicked_result(
        &mut self,
        path: &Path,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.clicked_result_index(path, generation) else {
            return;
        };
        self.selected_index = Some(index);
        self.load_selected_preview(cx);
        self.query.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn clicked_result_index(&self, path: &Path, generation: u64) -> Option<usize> {
        if self.searching
            || generation != self.search_generation
            || self.results_query.as_deref() != Some(self.search_query.as_str())
            || self.settings_open
            || self.creation.is_some()
            || self.quitting
        {
            return None;
        }
        self.results
            .iter()
            .position(|result| result.canonical_path == path)
    }

    fn row_edit_unavailable(&self, index: usize, cx: &App) -> Option<&'static str> {
        if self.searching {
            return Some("Wait for search results to finish updating");
        }
        if self.scan_config.is_none() {
            return Some("Editing is unavailable without active scan settings");
        }
        let Some(result) = self.results.get(index) else {
            return Some("This result is no longer available");
        };
        if self.editor.read(cx).path() == Some(result.canonical_path.as_path()) {
            return Some("Editing this note - Escape returns to search");
        }
        None
    }

    fn row_copy_unavailable(&self, index: usize, cx: &App) -> Option<&'static str> {
        if self.searching {
            return Some("Wait for search results to finish updating");
        }
        if self.editor.read(cx).path().is_some() {
            return Some("Finish editing before copying Markdown");
        }
        if self.results.get(index).is_none() || self.selected_index != Some(index) {
            return Some("Select this note and wait for its preview before copying");
        }
        if self.copying {
            return Some("Copying Markdown...");
        }
        if matches!(self.preview, PreviewState::Failed { .. }) {
            return Some("Copy unavailable because the preview could not be loaded");
        }
        if !self.selected_preview_ready() {
            return Some("Wait for a ready preview before copying");
        }
        None
    }

    fn edit_clicked_result(
        &mut self,
        path: &Path,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.clicked_result_index(path, generation) else {
            return;
        };
        if self.row_edit_unavailable(index, cx).is_some() {
            return;
        }
        self.activate_clicked_result(path, generation, window, cx);
        self.edit_selected(&EditSelected, window, cx);
    }

    fn copy_clicked_result(
        &mut self,
        path: &Path,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.clicked_result_index(path, generation) else {
            return;
        };
        if self.row_copy_unavailable(index, cx).is_some() {
            return;
        }
        self.query.update(cx, |input, cx| input.focus(window, cx));
        self.activate_selected(&ActivateSelected, window, cx);
    }

    fn complete_copy(
        &mut self,
        generation: u64,
        result: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.copy_generation != generation {
            return;
        }
        self.copying = false;
        match result {
            Ok(body) => {
                cx.write_to_clipboard(ClipboardItem::new_string(body));
                self.reset_query(window, cx);
                hide_application(cx);
            }
            Err(error) => {
                if let Some(service) = &self._watch_service {
                    service.request_rebuild();
                }
                self.action_status = Some(format!("Copy failed: {error}"));
                cx.notify();
            }
        }
    }

    fn cancel_copy(&mut self) {
        self.copying = false;
        self.preview_selection.dragging = false;
        self.copy_generation = self.copy_generation.wrapping_add(1);
        self.action_status = None;
    }

    fn rebuild_index(&mut self, _: &RebuildIndex, _: &mut Window, cx: &mut Context<Self>) {
        self.request_rebuild(cx);
    }

    fn request_rebuild(&mut self, cx: &mut Context<Self>) {
        if let Some(service) = &self._watch_service {
            service.request_rebuild();
            self.action_status = Some("Index rebuild requested...".into());
            cx.notify();
        }
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.show_settings(window, cx);
    }

    fn quit_application(&mut self, _: &QuitApplication, _: &mut Window, cx: &mut Context<Self>) {
        if self
            .creation
            .as_ref()
            .is_some_and(|view| view.read(cx).is_busy())
        {
            return;
        }
        self.quitting = true;
        self.cancel_copy();
        self.editor.update(cx, |editor, cx| editor.quit(cx));
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_copy();
        if self.editor.read(cx).path().is_some() {
            self.finish_edit(cx);
            self.query.update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
            return;
        }
        if self.preview_focus.is_focused(window) {
            self.query.update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
            return;
        }
        self.reset_query(window, cx);
        hide_application(cx);
    }

    fn reset_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_edit(cx);
        self.action_status = None;
        self.selected_index = None;
        self.clear_preview();
        self.query
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn scroll_to_selection(&self) {
        if let Some(index) = self.selected_index {
            self.scroll_handle
                .scroll_to_item(index, ScrollStrategy::Center);
        }
    }

    fn status_text(&self) -> SharedString {
        if let Some(status) = self
            .index_status
            .as_ref()
            .filter(|status| status.activity == IndexActivity::Rebuilding)
        {
            return format!(
                "Rebuilding: {} checked, {} updated",
                status.progress.processed_files, status.progress.indexed_files
            )
            .into();
        }
        if let Some(status) = &self.action_status {
            return status.clone().into();
        }
        if let Some(warning) = &self.creation_warning {
            return warning.clone().into();
        }
        if let Some(error) = &self.startup_error {
            return format!("Startup warning: {error}").into();
        }
        if let Some(error) = &self.search_error {
            return format!("Search error: {error}").into();
        }
        if let Some(error) = self
            .index_status
            .as_ref()
            .and_then(|status| status.last_error.as_ref())
        {
            return format!("Index warning: {error}").into();
        }
        if let Some(status) = &self.index_status {
            match status.activity {
                IndexActivity::InitialScan | IndexActivity::Reconciling => {
                    return format!(
                        "Indexing: {} checked, {} updated, {} issues",
                        status.progress.processed_files,
                        status.progress.indexed_files,
                        status.progress.issue_count
                    )
                    .into();
                }
                IndexActivity::Rebuilding | IndexActivity::Watching | IndexActivity::Stopping => {}
            }
        }
        if self.searching {
            return "Searching...".into();
        }
        if let Some(status) = &self.index_status {
            if !status.watcher_connected {
                return format!(
                    "{} results - filesystem watcher reconnecting",
                    self.results.len()
                )
                .into();
            }
            if status.issue_count > 0 {
                let detail = status
                    .recent_issues
                    .first()
                    .map(|issue| {
                        let file_name = issue
                            .path
                            .file_name()
                            .map_or_else(|| "unknown path".into(), |name| name.to_string_lossy());
                        format!(" - {file_name}: {}", truncate_text(&issue.message, 72))
                    })
                    .unwrap_or_default();
                return format!(
                    "{} results - {} indexing issues{}",
                    self.results.len(),
                    status.issue_count,
                    detail
                )
                .into();
            }
            if status.database_recovery.is_some() {
                return format!("{} results - recovered search index", self.results.len()).into();
            }
        }
        format!("{} results", self.results.len()).into()
    }

    fn begin_preview_selection(
        &mut self,
        offset: usize,
        event: &gpui_kit::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selected_preview_ready() {
            return;
        }
        self.preview_focus.focus(window, cx);
        self.preview_selection.begin(offset, event.modifiers.shift);
        if event.click_count == 2
            && let PreviewState::Ready { document, .. } = &self.preview
        {
            self.preview_selection
                .select_word(&document.selection_text, offset);
        }
        self.preview_drag_point = Some(event.position);
        self._selection_scroll_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(30))
                    .await;
                let keep_running = this
                    .update(cx, |launcher, cx| {
                        if !launcher.preview_selection.dragging {
                            return false;
                        }
                        if let Some(point) = launcher.preview_drag_point {
                            let bounds = launcher.preview_scroll.viewport_bounds();
                            let delta = if point.y < bounds.top() {
                                px(-24.0)
                            } else if point.y > bounds.bottom() {
                                px(24.0)
                            } else {
                                px(0.0)
                            };
                            if delta != px(0.0) {
                                launcher.preview_scroll.scroll_by(delta);
                                cx.notify();
                            }
                            launcher.update_preview_drag(cx);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
            }
        });
        cx.notify();
    }

    fn update_preview_drag(&mut self, cx: &mut Context<Self>) {
        let Some(point) = self.preview_drag_point else {
            return;
        };
        let PreviewState::Ready { document, .. } = &self.preview else {
            return;
        };
        let cursor = self
            .preview_layouts
            .borrow()
            .iter()
            .min_by(|(_, first), (_, second)| {
                let distance = |layout: &gpui_kit::TextLayout| {
                    let bounds = layout.bounds();
                    (point.y - bounds.bottom())
                        .max(bounds.top() - point.y)
                        .max(px(0.0))
                        .as_f32()
                };
                distance(first).total_cmp(&distance(second))
            })
            .and_then(|(index, layout)| {
                document
                    .blocks
                    .get(*index)
                    .map(|block| block.text_range.start + closest_preview_offset(layout, point))
            });
        if let Some(cursor) = cursor
            && self.preview_selection.cursor != cursor
        {
            self.preview_selection.cursor = cursor;
            cx.notify();
        }
    }

    fn select_all_preview(&mut self, _: &SelectAllPreview, _: &mut Window, cx: &mut Context<Self>) {
        if let PreviewState::Ready { document, .. } = &self.preview {
            self.preview_selection.select_all(&document.selection_text);
            cx.notify();
        }
    }

    fn copy_preview_selection(
        &mut self,
        _: &CopyPreviewSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let PreviewState::Ready { document, .. } = &self.preview {
            let text = self
                .preview_selection
                .selected_text(&document.selection_text);
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
            }
        }
    }

    fn render_preview_row(&self, index: usize, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let PreviewState::Ready { document, .. } = &self.preview else {
            return div().into_any_element();
        };
        let Some(block) = document.blocks.get(index) else {
            return div().into_any_element();
        };
        let selected = self.preview_selection.local_range(&block.text_range);
        let text = gpui_kit::StyledText::new(block.text.clone()).with_highlights(
            selected.into_iter().map(|range| {
                (
                    range,
                    gpui_kit::HighlightStyle {
                        background_color: Some(rgb(0x2563eb).into()),
                        ..Default::default()
                    },
                )
            }),
        );
        let layout = text.layout().clone();
        let painted_layout = layout.clone();
        let layouts = Rc::clone(&self.preview_layouts);
        let start = block.text_range.start;
        let generation = self.preview_generation.load(Ordering::Relaxed);
        div()
            .debug_selector(move || format!("preview-row-{index}"))
            .w_full()
            .text_sm()
            .text_color(rgb(0xf4f4f5))
            .cursor_text()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(
                    move |launcher, event: &gpui_kit::MouseDownEvent, window, cx| {
                        if generation != launcher.preview_generation.load(Ordering::Relaxed) {
                            return;
                        }
                        let offset = start + closest_preview_offset(&layout, event.position);
                        launcher.begin_preview_selection(offset, event, window, cx);
                    },
                ),
            )
            .when(block.ends_block, |row| row.pb_3())
            .child(
                div()
                    .w_full()
                    .when(block.kind == BlockKind::Heading, |row| {
                        row.text_lg().font_weight(gpui_kit::FontWeight::BOLD)
                    })
                    .when(block.kind == BlockKind::Code, |row| {
                        row.bg(rgb(0x27272a))
                            .px_3()
                            .when(block.starts_block, |row| row.pt_3())
                            .when(block.ends_block, |row| row.pb_3())
                    })
                    .relative()
                    .child(text)
                    .child(
                        gpui_kit::canvas(
                            move |_, _, _| {
                                // Overdraw rows may be measured without being prepainted.
                                // Register only layouts whose text has real window bounds.
                                layouts.borrow_mut().insert(index, painted_layout);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    ),
            )
            .into_any_element()
    }

    fn render_preview(&self, window: &Window, cx: &Context<Self>) -> gpui_kit::AnyElement {
        if self.editor.read(cx).path().is_some() {
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(self.editor.clone())
                .into_any_element();
        }
        self.preview_layouts.borrow_mut().clear();
        let mut pane = div()
            .debug_selector(|| "document-preview".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .gap_3()
            .border_l_1()
            .border_color(rgb(0x3f3f46))
            .pl_4()
            .track_focus(&self.preview_focus)
            .key_context(PREVIEW_KEY_CONTEXT)
            .on_action(cx.listener(Self::preview_page_up))
            .on_action(cx.listener(Self::preview_page_down))
            .on_action(cx.listener(Self::preview_start))
            .on_action(cx.listener(Self::preview_end))
            .on_action(cx.listener(Self::copy_preview_selection))
            .on_action(cx.listener(Self::select_all_preview));
        let path = match &self.preview {
            PreviewState::Empty => None,
            PreviewState::Loading(path)
            | PreviewState::Ready { path, .. }
            | PreviewState::Failed { path, .. } => Some(path),
        };
        let hint = if self.preview_focus.is_focused(window) {
            "Escape: search"
        } else {
            "Enter: focus preview"
        };
        if let Some(path) = path {
            pane = pane.child(document_header("Preview / Read-only", path, hint));
        } else {
            pane = pane.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xa1a1aa))
                    .child("Preview / Read-only"),
            );
        }
        match &self.preview {
            PreviewState::Empty => pane.child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa1a1aa))
                    .child("Select a note to preview using Up/Down or a click."),
            ),
            PreviewState::Loading(_) => pane.child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa1a1aa))
                    .child("Loading preview…"),
            ),
            PreviewState::Failed { error, .. } => {
                pane.child(div().text_sm().text_color(rgb(0xfca5a5)).child(format!(
                    "Preview unavailable: {}",
                    truncate_text(error, 400)
                )))
            }
            PreviewState::Ready { document, .. } => {
                if document.blocks.is_empty() {
                    pane = pane.child(div().text_sm().child("This note has no body content."));
                }
                pane.child(
                    div()
                        .id("preview-body")
                        .debug_selector(|| "preview-body".into())
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .overflow_hidden()
                        .child(
                            // Use visible-only measurement: measuring all rows would
                            // defeat virtualization on the first frame.
                            list(
                                self.preview_scroll.clone(),
                                cx.processor(|launcher, index, _, cx| {
                                    launcher.render_preview_row(index, cx)
                                }),
                            )
                            .size_full()
                            .pr_3(),
                        )
                        .child(gpui_kit::component::scroll::Scrollbar::vertical(
                            &self.preview_scroll,
                        )),
                )
            }
        }
        .into_any_element()
    }

    fn render_result_row(&mut self, index: usize, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let result = &self.results[index];
        let selected = self.selected_index == Some(index);
        let clicked_path = result.canonical_path.clone();
        let edit_path = clicked_path.clone();
        let copy_path = clicked_path.clone();
        let hover_group: SharedString = format!("result-hover-{index}").into();
        let edit_unavailable = self.row_edit_unavailable(index, cx);
        let copy_unavailable = self.row_copy_unavailable(index, cx);
        let editing = self.editor.read(cx).path() == Some(result.canonical_path.as_path());
        let generation = self.search_generation;
        let tags = if result.tags.is_empty() {
            String::new()
        } else {
            format!("  #{}", result.tags.join(" #"))
        };
        let full_path = result.canonical_path.to_string_lossy().into_owned();
        let path = display_result_path(&result.canonical_path, self.home_directory.as_deref());
        let excerpt = result.excerpt.replace(['\n', '\r'], " ");

        div()
            .id(SharedString::from(
                result.canonical_path.to_string_lossy().into_owned(),
            ))
            .debug_selector(move || format!("result-{index}"))
            .role(gpui_kit::accesskit::Role::ListBoxOption)
            .aria_selected(selected)
            .w_full()
            .h(px(88.0))
            .group(hover_group.clone())
            .flex()
            .items_center()
            .px_3()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |row| row.bg(rgb(0x27272a)))
            .hover(move |row| if selected { row } else { row.bg(rgb(0x222225)) })
            .on_click(cx.listener(move |launcher, _, window, cx| {
                launcher.activate_clicked_result(&clicked_path, generation, window, cx);
            }))
            .child(
                div()
                    .id(("result-content", index))
                    .tooltip(move |window, cx| Tooltip::new(full_path.clone()).build(window, cx))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .debug_selector(move || format!("result-content-{index}"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xf4f4f5))
                            .text_ellipsis()
                            .child(format!("{}{}", result.title, tags)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .debug_selector(move || format!("result-path-{index}"))
                            .text_color(rgb(0xa1a1aa))
                            .text_ellipsis()
                            .child(path),
                    )
                    .when(!excerpt.trim().is_empty(), |content| {
                        content.child(
                            div()
                                .debug_selector(move || format!("result-excerpt-{index}"))
                                .text_xs()
                                .text_color(rgb(0xa1a1aa))
                                .text_ellipsis()
                                .child(excerpt),
                        )
                    }),
            )
            .child(
                div()
                    .id(("result-actions", index))
                    .debug_selector(move || format!("result-actions-{index}"))
                    .flex()
                    .flex_shrink_0()
                    .w(px(56.0))
                    .gap_1()
                    .when(!selected, |actions| {
                        actions
                            .invisible()
                            .group_hover(hover_group, |style| style.visible())
                    })
                    // Disabled buttons must not bubble into row selection either.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, window, _| {
                        window.prevent_default()
                    })
                    .child(
                        action_button(result_action_id("edit", &result.canonical_path), cx)
                            .small()
                            .icon(if editing {
                                IconName::Check
                            } else {
                                IconName::SquarePen
                            })
                            .accessibility_label(if editing {
                                "Editing note".into()
                            } else if let Some(reason) = edit_unavailable {
                                format!("Edit unavailable: {reason}")
                            } else {
                                "Edit note".into()
                            })
                            .tooltip(
                                edit_unavailable
                                    .unwrap_or("Edit this note (Cmd/Ctrl+E for selected note)"),
                            )
                            .disabled(edit_unavailable.is_some())
                            .tab_stop(selected && edit_unavailable.is_none())
                            .when(edit_unavailable.is_some(), |button| button.opacity(0.45))
                            .on_click(cx.listener(move |launcher, _, window, cx| {
                                cx.stop_propagation();
                                launcher.edit_clicked_result(&edit_path, generation, window, cx);
                            })),
                    )
                    .child(
                        action_button(result_action_id("copy", &result.canonical_path), cx)
                            .small()
                            .icon(IconName::Copy)
                            .accessibility_label(
                                copy_unavailable
                                    .map(|reason| format!("Copy unavailable: {reason}"))
                                    .unwrap_or_else(|| "Copy Markdown body".into()),
                            )
                            .tooltip(
                                copy_unavailable
                                    .unwrap_or("Copy Markdown body and hide (Cmd/Ctrl+Shift+C)"),
                            )
                            .disabled(copy_unavailable.is_some())
                            .tab_stop(selected && copy_unavailable.is_none())
                            .when(copy_unavailable.is_some(), |button| button.opacity(0.45))
                            .on_click(cx.listener(move |launcher, _, window, cx| {
                                cx.stop_propagation();
                                launcher.copy_clicked_result(&copy_path, generation, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.quitting {
            return div()
                .size_full()
                .bg(rgb(0x18181b))
                .text_color(rgb(0xf4f4f5))
                .p_3()
                .child("Finishing pending saves before quitting...");
        }
        if let Some(view) = &self.creation {
            return div()
                .size_full()
                .on_action(cx.listener(Self::quit_application))
                .child(view.clone());
        }
        let result_count = self.results.len();
        let status = self
            .editor
            .read(cx)
            .notice()
            .map(SharedString::from)
            .unwrap_or_else(|| self.status_text());

        div()
            .track_focus(&self.focus)
            .key_context(if self.editor.read(cx).is_focused(window, cx) {
                EDITOR_KEY_CONTEXT
            } else {
                KEY_CONTEXT
            })
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::activate_selected))
            .on_action(cx.listener(Self::focus_preview))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::rebuild_index))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::new_note))
            .on_action(cx.listener(Self::edit_selected))
            .on_action(cx.listener(Self::save_editor))
            .on_action(cx.listener(Self::quit_application))
            .on_mouse_move(
                cx.listener(|launcher, event: &gpui_kit::MouseMoveEvent, _, cx| {
                    if launcher.preview_selection.dragging {
                        launcher.preview_drag_point = Some(event.position);
                        launcher.update_preview_drag(cx);
                    }
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|launcher, _, _, _| {
                    launcher.preview_selection.dragging = false;
                }),
            )
            .on_mouse_up_out(
                gpui_kit::MouseButton::Left,
                cx.listener(|launcher, _, _, _| {
                    launcher.preview_selection.dragging = false;
                }),
            )
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x18181b))
            .text_color(rgb(0xf4f4f5))
            .p_3()
            .gap_2()
            .child(
                div()
                    .id("launcher-toolbar")
                    .role(gpui_kit::accesskit::Role::Toolbar)
                    .aria_label("Search and note creation")
                    .debug_selector(|| "launcher-search-bar".into())
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .debug_selector(|| "launcher-query".into())
                            .flex_1()
                            .min_w_0()
                            .child(
                                Input::new(&self.query)
                                    .text_color(rgb(0x000000))
                                    .cleanable(true)
                                    .aria_label("Search Markdown files"),
                            ),
                    )
                    .child(
                        primary_button("workspace-new-note", cx)
                            .icon(IconName::Plus)
                            .label("New")
                            .accessibility_label(if self.scan_config.is_some() {
                                "New Markdown note"
                            } else {
                                "New note unavailable without active scan settings"
                            })
                            .flex_shrink_0()
                            .tooltip(if self.scan_config.is_some() {
                                "New note (Cmd/Ctrl+N)"
                            } else {
                                "Note creation is unavailable without active scan settings"
                            })
                            .disabled(self.scan_config.is_none())
                            .tab_stop(self.scan_config.is_some())
                            .when(self.scan_config.is_none(), |button| button.opacity(0.45))
                            .on_click(cx.listener(|launcher, _, window, cx| {
                                cx.stop_propagation();
                                launcher.new_note(&NewNote, window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .child(
                        div()
                            .id("search-results-listbox")
                            .role(gpui_kit::accesskit::Role::ListBox)
                            .aria_label("Markdown search results")
                            .w(gpui_kit::relative(0.35))
                            .h_full()
                            .flex_shrink_0()
                            .child(
                                uniform_list(
                                    "search-results",
                                    result_count,
                                    cx.processor(
                                        |launcher, range: std::ops::Range<usize>, _, cx| {
                                            range
                                                .map(|index| launcher.render_result_row(index, cx))
                                                .collect::<Vec<_>>()
                                        },
                                    ),
                                )
                                .debug_selector(|| "search-results".into())
                                .size_full()
                                .track_scroll(&self.scroll_handle),
                            ),
                    )
                    .child(self.render_preview(window, cx)),
            )
            .child(
                div()
                    .id("workspace-status")
                    .role(gpui_kit::accesskit::Role::Status)
                    .aria_label(status.clone())
                    .debug_selector(|| "workspace-status".into())
                    .max_h(px(96.0))
                    .overflow_y_scroll()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(rgb(0xa1a1aa))
                    .child(format!(
                        "{} · {}",
                        status,
                        if self.editor.read(cx).path().is_some() {
                            "Cmd/Ctrl+S: save · Escape: search"
                        } else {
                            "Cmd/Ctrl+N: new · Cmd/Ctrl+E: edit · Cmd/Ctrl+Shift+C: copy note"
                        }
                    ))
                    .test_support(),
            )
    }
}

#[cfg(target_os = "macos")]
fn hide_launcher_for_settings(window: &Window) -> Result<(), String> {
    macos_window_visibility::hide_window(window)
}

#[cfg(not(target_os = "macos"))]
fn hide_launcher_for_settings(_: &Window) -> Result<(), String> {
    Ok(())
}

#[cfg(not(test))]
fn hide_application(cx: &mut App) {
    cx.hide();
}

#[cfg(test)]
fn hide_application(_: &mut App) {}

fn result_action_id(action: &str, path: &Path) -> SharedString {
    format!("result-{action}:{}", path.display()).into()
}

fn display_result_path(path: &Path, home: Option<&Path>) -> String {
    if let Some(relative) = home.and_then(|home| path.strip_prefix(home).ok()) {
        if relative.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~/{}", relative.display());
    }
    path.to_string_lossy().into_owned()
}

fn closest_preview_offset(
    layout: &gpui_kit::TextLayout,
    point: gpui_kit::Point<gpui_kit::Pixels>,
) -> usize {
    let mut origin = layout.bounds().origin;
    if point.y < origin.y {
        return 0;
    }
    let height = layout.line_height();
    let mut offset = 0;
    for line in layout.line_layouts() {
        let line_height = line.size(height).height;
        if point.y < origin.y + line_height {
            return offset
                + line
                    .closest_index_for_position(point - origin, height)
                    .unwrap_or_else(|index| index);
        }
        origin.y += line_height;
        offset += line.len() + 1;
    }
    layout.len()
}

fn truncate_text(value: &str, maximum_chars: usize) -> String {
    let mut characters = value.chars();
    let shortened = characters.by_ref().take(maximum_chars).collect::<String>();
    if characters.next().is_some() {
        format!("{shortened}...")
    } else {
        shortened
    }
}

fn is_current_search_generation(current: u64, completed: u64) -> bool {
    current == completed
}

fn selection_after_refresh(
    current: Option<usize>,
    previous: &[SearchResult],
    refreshed: &[SearchResult],
    preserve: bool,
) -> Option<usize> {
    if !preserve {
        return None;
    }
    current
        .and_then(|index| previous.get(index))
        .and_then(|selected| {
            refreshed
                .iter()
                .position(|result| result.canonical_path == selected.canonical_path)
        })
}

fn previous_selection(current: Option<usize>, result_count: usize) -> Option<usize> {
    (result_count > 0).then(|| current.map_or(result_count - 1, |index| index.saturating_sub(1)))
}

fn next_selection(current: Option<usize>, result_count: usize) -> Option<usize> {
    (result_count > 0).then(|| {
        current
            .map_or(0, |index| index.saturating_add(1))
            .min(result_count - 1)
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use gpui_kit::{
        AnyWindowHandle, Bounds, Entity, Focusable as _, Modifiers, MouseButton, TestAppContext,
        VisualTestContext, WindowBounds, WindowOptions, point, size,
    };

    use crate::{config::ScanConfig, index::MarkdownIndexer};

    use super::*;

    #[test]
    fn diagnostic_text_is_bounded() {
        assert_eq!(truncate_text("short", 10), "short");
        assert_eq!(truncate_text("long diagnostic", 4), "long...");
    }

    #[test]
    fn stale_search_generations_are_rejected() {
        assert!(is_current_search_generation(3, 3));
        assert!(!is_current_search_generation(3, 2));
    }

    #[test]
    fn selection_navigation_is_safe_for_empty_results() {
        assert_eq!(previous_selection(None, 0), None);
        assert_eq!(next_selection(None, 0), None);
    }

    #[test]
    fn selection_navigation_stays_inside_the_result_set() {
        assert_eq!(previous_selection(None, 3), Some(2));
        assert_eq!(previous_selection(Some(0), 3), Some(0));
        assert_eq!(previous_selection(Some(2), 3), Some(1));
        assert_eq!(next_selection(None, 3), Some(0));
        assert_eq!(next_selection(Some(2), 3), Some(2));
    }

    #[test]
    fn selection_refresh_preserves_only_an_explicit_surviving_file() {
        let result = |path: &str| SearchResult {
            canonical_path: path.into(),
            title: "README".into(),
            tags: vec![],
            excerpt: String::new(),
            score: 0.0,
        };
        let previous = vec![
            result("/a/README.md"),
            result("/b/README.md"),
            result("/c/README.md"),
        ];
        let refreshed = vec![
            result("/c/README.md"),
            result("/a/README.md"),
            result("/b/README.md"),
        ];
        assert_eq!(
            selection_after_refresh(Some(1), &previous, &refreshed, true),
            Some(2)
        );
        assert_eq!(
            selection_after_refresh(Some(2), &previous, &previous[..2], true),
            None
        );
        assert_eq!(
            selection_after_refresh(Some(1), &previous, &refreshed, false),
            None
        );
        assert_eq!(selection_after_refresh(Some(1), &previous, &[], true), None);
        assert_eq!(
            selection_after_refresh(None, &previous, &refreshed, true),
            None
        );
    }

    #[gpui_kit::test]
    fn background_refreshes_preserve_navigation_and_handle_removed_results(
        cx: &mut TestAppContext,
    ) {
        let (temporary_directory, database_path, window, launcher) = open_indexed_test_launcher(
            cx,
            &[
                ("alpha.md", "alpha"),
                ("beta.md", "beta"),
                ("gamma.md", "gamma"),
            ],
        );
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down down");
        cx.update(|cx| assert_eq!(launcher.read(cx).selected_index, Some(1)));
        window
            .update(cx, |_, window, cx| {
                launcher.update(cx, |launcher, cx| {
                    launcher.start_search(String::new(), cx);
                    launcher.select_next(&SelectNext, window, cx);
                })
            })
            .expect("navigate while refreshing");
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(launcher.read(cx).selected_index, Some(2)));

        let notes = temporary_directory.path().join("notes");
        fs::write(notes.join("aardvark.md"), "new first result").expect("new note");
        let config = ScanConfig::default_for_home(&notes);
        let mut database = IndexDatabase::open(&database_path).expect("database");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("reindex");
        launcher.update(cx, |launcher, cx| launcher.start_search(String::new(), cx));
        cx.run_until_parked();
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.selected_index, Some(3));
            assert!(launcher.results[3].canonical_path.ends_with("gamma.md"));
        });
        fs::remove_file(notes.join("gamma.md")).expect("remove selected note");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("reindex removal");
        launcher.update(cx, |launcher, cx| launcher.start_search(String::new(), cx));
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(launcher.read(cx).selected_index, None);
            assert_eq!(launcher.read(cx).preview, PreviewState::Empty);
        });
        cx.simulate_keystrokes(window, "alpha");
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.selected_index, None);
            assert!(launcher.results[0].canonical_path.ends_with("alpha.md"));
        });
    }

    #[gpui_kit::test]
    fn searches_navigates_and_copies_the_current_file(cx: &mut TestAppContext) {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let notes = temporary_directory.path().join("notes");
        fs::create_dir(&notes).expect("create notes directory");
        fs::write(notes.join("alpha.md"), "alpha body").expect("write alpha");
        let beta_path = notes.join("beta.md");
        fs::write(&beta_path, "---\ntitle: Beta\n---\nindexed body\n").expect("write beta");
        let database_path = temporary_directory.path().join("index.sqlite3");
        let config = ScanConfig::default_for_home(&notes);
        let mut database = IndexDatabase::open(&database_path).expect("database");
        MarkdownIndexer::new(&config, &mut database)
            .run()
            .expect("index documents");
        drop(database);
        let (window, launcher) = open_test_launcher(cx, database_path);

        cx.run_until_parked();
        cx.dispatch_action(window, SelectNext);
        cx.dispatch_action(window, SelectNext);
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.results.len(), 2);
            assert_eq!(launcher.selected_index, Some(1));
        });
        cx.dispatch_action(window, SelectPrevious);
        cx.update(|cx| assert_eq!(launcher.read(cx).selected_index, Some(0)));

        cx.simulate_keystrokes(window, "beta");
        cx.run_until_parked();
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.results.len(), 1);
            assert_eq!(launcher.results[0].title, "Beta");
            assert_eq!(launcher.selected_index, None);
        });
        cx.dispatch_action(window, SelectNext);
        settle_preview(cx);
        fs::write(&beta_path, "---\ntitle: Beta\n---\nlatest body\n").expect("update beta");

        cx.dispatch_action(window, ActivateSelected);
        cx.run_until_parked();

        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("latest body\n".into())
        );
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.query.read(cx).value(), "");
        });
    }

    #[gpui_kit::test]
    fn empty_results_and_escape_do_not_change_the_clipboard(cx: &mut TestAppContext) {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let database_path = temporary_directory.path().join("index.sqlite3");
        drop(IndexDatabase::open(&database_path).expect("database"));
        let (window, launcher) = open_test_launcher(cx, database_path);
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        cx.run_until_parked();

        cx.dispatch_action(window, ActivateSelected);
        cx.dispatch_action(window, Dismiss);
        cx.run_until_parked();

        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.results.is_empty());
            assert_eq!(launcher.selected_index, None);
        });
    }

    #[gpui_kit::test]
    fn pending_search_cannot_copy_a_previous_result(cx: &mut TestAppContext) {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let notes = temporary_directory.path().join("notes");
        fs::create_dir(&notes).expect("notes");
        fs::write(notes.join("alpha.md"), "previous result body").expect("note");
        let database_path = temporary_directory.path().join("index.sqlite3");
        let mut database = IndexDatabase::open(&database_path).expect("database");
        MarkdownIndexer::new(&ScanConfig::default_for_home(&notes), &mut database)
            .run()
            .expect("index");
        drop(database);
        let (window, launcher) = open_test_launcher(cx, database_path);
        cx.run_until_parked();
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        window
            .update(cx, |_, window, cx| {
                launcher.update(cx, |launcher, cx| {
                    launcher.start_search("nothingmatches".into(), cx);
                    assert_eq!(launcher.selected_index, None);
                    launcher.select_next(&SelectNext, window, cx);
                    launcher.activate_selected(&ActivateSelected, window, cx);
                    assert!(launcher.action_status.is_none());
                });
            })
            .expect("activate during search");
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        cx.update(|cx| assert!(launcher.read(cx).results.is_empty()));
    }

    #[gpui_kit::test]
    fn escape_invalidates_an_in_flight_copy(cx: &mut TestAppContext) {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let notes = temporary_directory.path().join("notes");
        fs::create_dir(&notes).expect("create notes");
        fs::write(notes.join("note.md"), "must not be copied").expect("write note");
        let database_path = temporary_directory.path().join("index.sqlite3");
        let mut database = IndexDatabase::open(&database_path).expect("database");
        MarkdownIndexer::new(&ScanConfig::default_for_home(&notes), &mut database)
            .run()
            .expect("index note");
        drop(database);
        let (window, launcher) = open_test_launcher(cx, database_path);
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        cx.run_until_parked();
        cx.dispatch_action(window, SelectNext);
        settle_preview(cx);
        window
            .update(cx, |_, window, cx| {
                launcher.update(cx, |launcher, cx| {
                    launcher.activate_selected(&ActivateSelected, window, cx);
                    assert!(launcher.action_status.is_some());
                    launcher.dismiss(&Dismiss, window, cx);
                });
            })
            .expect("dismiss before the copy task completes");
        cx.run_until_parked();

        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        cx.update(|cx| assert!(launcher.read(cx).action_status.is_none()));
    }

    #[gpui_kit::test]
    fn canceled_copy_completions_do_not_affect_a_new_session(cx: &mut TestAppContext) {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let database_path = temporary_directory.path().join("index.sqlite3");
        drop(IndexDatabase::open(&database_path).expect("database"));
        let (window, launcher) = open_test_launcher(cx, database_path);
        cx.run_until_parked();
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        cx.dispatch_action(window, Dismiss);
        cx.simulate_keystrokes(window, "new space session");
        cx.run_until_parked();

        window
            .update(cx, |_, window, cx| {
                launcher.update(cx, |launcher, cx| {
                    let canceled_generation = launcher.copy_generation.wrapping_sub(1);
                    launcher.complete_copy(
                        canceled_generation,
                        Ok("stale body".into()),
                        window,
                        cx,
                    );
                    launcher.complete_copy(
                        canceled_generation,
                        Err("stale error".into()),
                        window,
                        cx,
                    );
                    assert_eq!(launcher.query.read(cx).value(), "new session");
                    assert!(launcher.action_status.is_none());
                });
            })
            .expect("update launcher window");

        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
    }

    #[gpui_kit::test]
    fn hovering_does_not_select_and_clicking_previews_the_latest_body(cx: &mut TestAppContext) {
        let (temporary_directory, _, window, launcher) = open_indexed_test_launcher(
            cx,
            &[
                ("note-alpha.md", "alpha body"),
                ("note-beta.md", "indexed beta"),
            ],
        );
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "note");
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        let mut visual = VisualTestContext::from_window(window, cx);
        let bounds = visual
            .debug_bounds("result-1")
            .expect("second result bounds");
        visual.simulate_mouse_move(bounds.center(), None, Modifiers::none());
        cx.update(|cx| assert_eq!(launcher.read(cx).selected_index, None));
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        visual.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::none());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Right, Modifiers::none());
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        fs::write(
            temporary_directory.path().join("notes/note-beta.md"),
            "latest beta",
        )
        .expect("update selected file");
        visual.simulate_click(bounds.center(), Modifiers::none());
        settle_preview(cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.query.read(cx).value(), "note");
            assert_eq!(launcher.selected_index, Some(1));
            assert!(
                matches!(&launcher.preview, PreviewState::Ready { document, .. }
                if document.blocks[0].text == "latest beta")
            );
        });
    }

    #[gpui_kit::test]
    fn short_result_rows_fill_the_list_and_accept_right_edge_clicks(cx: &mut TestAppContext) {
        let (temporary_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("a.md", "a"), ("b.md", "b"), ("c.md", "")]);
        cx.run_until_parked();
        launcher.update(cx, |launcher, cx| {
            launcher.home_directory = Some(temporary_directory.path().join("notes"));
            cx.notify();
        });
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(window, cx);
        let list = visual.debug_bounds("search-results").expect("list bounds");
        for selector in ["result-0", "result-1", "result-2"] {
            let row = visual.debug_bounds(selector).expect("row bounds");
            assert_eq!(row.origin.x, list.origin.x);
            assert_eq!(
                row.size.width, list.size.width,
                "short rows must fill the list"
            );
        }
        let row = visual.debug_bounds("result-1").expect("second row");
        let right_edge = point(list.right() - px(4.0), row.center().y);
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        visual.simulate_mouse_move(right_edge, None, Modifiers::none());
        cx.update(|cx| assert_eq!(launcher.read(cx).selected_index, None));
        visual.simulate_click(right_edge, Modifiers::none());
        settle_preview(cx);
        cx.update(|cx| assert_eq!(launcher.read(cx).selected_index, Some(1)));
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
    }

    #[gpui_kit::test]
    fn failed_preview_reads_leave_the_query_and_clipboard_unchanged(cx: &mut TestAppContext) {
        let (temporary_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", "body")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "note");
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        fs::remove_file(temporary_directory.path().join("notes/note.md")).expect("remove note");
        let mut visual = VisualTestContext::from_window(window, cx);
        let bounds = visual.debug_bounds("result-0").expect("result bounds");
        visual.simulate_click(bounds.center(), Modifiers::none());
        settle_preview(cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.query.read(cx).value(), "note");
            assert!(matches!(launcher.preview, PreviewState::Failed { .. }));
        });
    }

    #[gpui_kit::test]
    fn clicks_from_pending_or_obsolete_results_are_ignored(cx: &mut TestAppContext) {
        let (_temporary_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", "body")]);
        cx.run_until_parked();
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        let (path, old_generation) = cx.update(|cx| {
            let launcher = launcher.read(cx);
            (
                launcher.results[0].canonical_path.clone(),
                launcher.search_generation,
            )
        });
        window
            .update(cx, |_, window, cx| {
                launcher.update(cx, |launcher, cx| {
                    launcher.start_search(String::new(), cx);
                    launcher.activate_clicked_result(&path, launcher.search_generation, window, cx);
                    assert!(launcher.action_status.is_none());
                })
            })
            .expect("click while refreshing");
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                launcher.update(cx, |launcher, cx| {
                    launcher.activate_clicked_result(&path, old_generation, window, cx);
                    assert!(launcher.action_status.is_none());
                    launcher.activate_clicked_result(&path, launcher.search_generation, window, cx);
                    launcher.dismiss(&Dismiss, window, cx);
                })
            })
            .expect("cancel current click");
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
    }

    #[test]
    fn result_paths_abbreviate_only_the_home_directory_boundary() {
        let home = Path::new("/home/user");
        assert_eq!(
            display_result_path(Path::new("/home/user/project/README.md"), Some(home)),
            "~/project/README.md"
        );
        assert_eq!(
            display_result_path(Path::new("/home/user-other/README.md"), Some(home)),
            "/home/user-other/README.md"
        );
        assert_eq!(display_result_path(home, Some(home)), "~");
        assert_eq!(
            display_result_path(Path::new("/notes/README.md"), None),
            "/notes/README.md"
        );
    }

    #[gpui_kit::test]
    fn readme_results_show_paths_even_when_they_have_excerpts(cx: &mut TestAppContext) {
        let (_temporary_directory, _, window, launcher) = open_indexed_test_launcher(
            cx,
            &[
                ("project-a/README.md", "same excerpt"),
                ("project-b/README.md", "same excerpt"),
            ],
        );
        cx.run_until_parked();
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.results[0].title, "README");
            assert_eq!(launcher.results[1].title, "README");
            assert!(!launcher.results[0].excerpt.is_empty());
            assert_ne!(
                display_result_path(
                    &launcher.results[0].canonical_path,
                    launcher.home_directory.as_deref()
                ),
                display_result_path(
                    &launcher.results[1].canonical_path,
                    launcher.home_directory.as_deref()
                )
            );
        });
        let mut visual = VisualTestContext::from_window(window, cx);
        for selector in [
            "result-path-0",
            "result-path-1",
            "result-excerpt-0",
            "result-excerpt-1",
        ] {
            let bounds = visual
                .debug_bounds(selector)
                .expect("visible path or excerpt");
            assert!(bounds.size.height > px(0.0));
        }
    }

    #[gpui_kit::test]
    fn search_never_selects_or_loads_a_document_automatically(cx: &mut TestAppContext) {
        let (_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
        settle_preview(cx);
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        cx.dispatch_action(window, ActivateSelected);
        cx.update(|cx| {
            assert_eq!(launcher.read(cx).selected_index, None);
            assert_eq!(launcher.read(cx).preview, PreviewState::Empty);
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        cx.dispatch_action(window, SelectPrevious);
        settle_preview(cx);
        cx.update(|cx| {
            assert_eq!(launcher.read(cx).selected_index, Some(1));
            assert!(
                matches!(&launcher.read(cx).preview, PreviewState::Ready { document, .. }
                if document.blocks[0].text == "beta")
            );
        });
        launcher.update(cx, |launcher, cx| {
            launcher.start_search("alpha".into(), cx);
            assert_eq!(launcher.preview, PreviewState::Empty);
        });
        settle_preview(cx);
        cx.update(|cx| {
            assert_eq!(launcher.read(cx).selected_index, None);
            assert_eq!(launcher.read(cx).preview, PreviewState::Empty);
        });
    }

    #[gpui_kit::test]
    fn obsolete_preview_successes_and_errors_cannot_replace_the_current_selection(
        cx: &mut TestAppContext,
    ) {
        let (_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
        cx.run_until_parked();
        cx.dispatch_action(window, SelectNext);
        let (old_generation, old_path) = cx.update(|cx| {
            let launcher = launcher.read(cx);
            (
                launcher.preview_generation.load(Ordering::Relaxed),
                launcher.results[0].canonical_path.clone(),
            )
        });
        cx.dispatch_action(window, SelectNext);
        launcher.update(cx, |launcher, cx| {
            launcher.complete_preview(old_generation, old_path.clone(), Ok(PreviewDocument {
                ..PreviewDocument::default()
            }), cx);
            launcher.complete_preview(old_generation, old_path, Err("obsolete failure".into()), cx);
            assert!(matches!(&launcher.preview, PreviewState::Loading(path) if path.ends_with("beta.md")));
        });
        settle_preview(cx);
        launcher.update(cx, |launcher, cx| {
            assert!(
                matches!(&launcher.preview, PreviewState::Ready { document, .. }
                if document.blocks[0].text == "beta")
            );
            let generation = launcher.preview_generation.load(Ordering::Relaxed);
            let path = launcher.results[1].canonical_path.clone();
            launcher.start_search("nomatches".into(), cx);
            launcher.complete_preview(generation, path, Err("late error".into()), cx);
            assert_eq!(launcher.preview, PreviewState::Empty);
        });
        settle_preview(cx);
        cx.update(|cx| assert_eq!(launcher.read(cx).preview, PreviewState::Empty));
    }

    #[gpui_kit::test]
    fn same_query_index_refresh_reloads_an_explicit_preview(cx: &mut TestAppContext) {
        let (directory, database_path, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", "before")]);
        cx.run_until_parked();
        cx.dispatch_action(window, SelectNext);
        settle_preview(cx);
        let notes = directory.path().join("notes");
        fs::write(notes.join("note.md"), "after external edit").expect("edit");
        let mut database = IndexDatabase::open(&database_path).expect("database");
        MarkdownIndexer::new(&ScanConfig::default_for_home(&notes), &mut database)
            .run()
            .expect("reindex");
        launcher.update(cx, |launcher, cx| launcher.start_search(String::new(), cx));
        settle_preview(cx);
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.selected_index, Some(0));
            assert!(
                matches!(&launcher.preview, PreviewState::Ready { document, .. }
                if document.blocks[0].text == "after external edit")
            );
        });
    }

    #[gpui_kit::test]
    fn virtual_preview_renders_visible_rows_and_can_scroll_to_the_end(cx: &mut TestAppContext) {
        let body = format!("{}final sentinel", "paragraph\n\n".repeat(3000));
        let (_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", &body)]);
        cx.run_until_parked();
        cx.dispatch_action(window, SelectNext);
        settle_preview(cx);
        let mut visual = VisualTestContext::from_window(window, cx);
        assert!(visual.debug_bounds("preview-row-0").is_some());
        assert!(
            visual.debug_bounds("preview-row-3000").is_none(),
            "offscreen tail must not be laid out initially"
        );
        let viewport = visual
            .debug_bounds("preview-body")
            .expect("preview viewport");
        visual.simulate_event(gpui_kit::ScrollWheelEvent {
            position: viewport.center(),
            delta: gpui_kit::ScrollDelta::Pixels(point(px(0.0), px(-300.0))),
            ..Default::default()
        });
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.preview_scroll.logical_scroll_top().item_ix > 0);
            assert_eq!(launcher.selected_index, Some(0));
        });
        launcher.update(cx, |launcher, cx| {
            let PreviewState::Ready { document, .. } = &launcher.preview else {
                panic!("loaded preview")
            };
            assert_eq!(document.blocks.len(), 3001);
            assert_eq!(document.blocks[3000].text, "final sentinel");
            launcher.preview_scroll.scroll_to_end();
            cx.notify();
        });
        cx.run_until_parked();
        assert!(
            visual.debug_bounds("preview-row-3000").is_some(),
            "last document row must be reachable"
        );
        assert!(
            visual.debug_bounds("preview-row-0").is_none(),
            "offscreen first row must be unmounted"
        );
        let top = cx.update(|cx| launcher.read(cx).preview_scroll.logical_scroll_top());
        launcher.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let after = cx.update(|cx| launcher.read(cx).preview_scroll.logical_scroll_top());
        assert_eq!(after.item_ix, top.item_ix);
        assert_eq!(after.offset_in_item, top.offset_in_item);
    }

    #[gpui_kit::test]
    fn selecting_another_document_resets_the_virtual_preview_scroll(cx: &mut TestAppContext) {
        let body = "paragraph\n\n".repeat(1000);
        let (_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("a.md", &body), ("b.md", "new note")]);
        cx.run_until_parked();
        cx.dispatch_action(window, SelectNext);
        settle_preview(cx);
        launcher.update(cx, |launcher, cx| {
            launcher.preview_scroll.scroll_to_end();
            cx.notify();
        });
        cx.run_until_parked();
        cx.dispatch_action(window, SelectNext);
        settle_preview(cx);
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.preview_scroll.logical_scroll_top().item_ix, 0);
            assert!(
                matches!(&launcher.preview, PreviewState::Ready { document, .. }
                if document.blocks[0].text == "new note")
            );
        });
        let mut visual = VisualTestContext::from_window(window, cx);
        assert!(visual.debug_bounds("preview-row-0").is_some());
    }

    #[gpui_kit::test]
    fn full_preview_does_not_change_clipboard_content(cx: &mut TestAppContext) {
        let body = "x".repeat(30_000);
        let (_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", &body)]);
        cx.run_until_parked();
        cx.dispatch_action(window, SelectNext);
        settle_preview(cx);
        cx.update(|cx| {
            assert!(matches!(&launcher.read(cx).preview,
            PreviewState::Ready { document, .. } if document.blocks.iter().map(|block| block.text.len()).sum::<usize>() == body.len()))
        });
        cx.dispatch_action(window, ActivateSelected);
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(body)
        );
    }

    #[gpui_kit::test]
    fn preview_respects_file_limit_and_escape_cancels_pending_load(cx: &mut TestAppContext) {
        let (_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", "body")]);
        cx.run_until_parked();
        launcher.update(cx, |launcher, _| launcher.max_file_size_bytes = 1);
        cx.dispatch_action(window, SelectNext);
        settle_preview(cx);
        cx.update(|cx| {
            assert!(matches!(
                &launcher.read(cx).preview,
                PreviewState::Failed { .. }
            ))
        });
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        cx.simulate_keystrokes(window, "enter cmd-shift-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        cx.dispatch_action(window, Dismiss);
        cx.run_until_parked();
        cx.dispatch_action(window, SelectNext);
        cx.dispatch_action(window, Dismiss);
        settle_preview(cx);
        cx.update(|cx| assert_eq!(launcher.read(cx).preview, PreviewState::Empty));
    }

    #[gpui_kit::test]
    fn results_and_preview_have_separate_non_overlapping_panes(cx: &mut TestAppContext) {
        let (_directory, _, window, _) = open_indexed_test_launcher(cx, &[("note.md", "body")]);
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(window, cx);
        let list = visual.debug_bounds("search-results").expect("list");
        let preview = visual.debug_bounds("document-preview").expect("preview");
        assert!(list.right() <= preview.origin.x);
        assert!(preview.size.width > list.size.width);
        assert!(preview.size.height > px(200.0));
    }

    #[gpui_kit::test]
    fn enter_focuses_preview_and_escape_returns_to_search_without_losing_state(
        cx: &mut TestAppContext,
    ) {
        let body = "paragraph\n\n".repeat(200);
        let (_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", &body)]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "note");
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down");
        settle_preview(cx);
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        cx.simulate_keystrokes(window, "enter down pagedown");
        window
            .update(cx, |_, window, cx| {
                let launcher = launcher.read(cx);
                assert!(launcher.preview_focus.is_focused(window));
                assert_eq!(launcher.selected_index, Some(0));
                assert_eq!(launcher.query.read(cx).value(), "note");
                assert!(launcher.preview_scroll.logical_scroll_top().item_ix > 0);
            })
            .expect("preview focus");
        cx.simulate_keystrokes(window, "escape");
        window
            .update(cx, |_, window, cx| {
                let launcher = launcher.read(cx);
                assert!(!launcher.preview_focus.is_focused(window));
                assert_eq!(launcher.selected_index, Some(0));
                assert_eq!(launcher.query.read(cx).value(), "note");
            })
            .expect("search focus");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        cx.simulate_keystrokes(window, "escape");
        cx.update(|cx| assert_eq!(launcher.read(cx).query.read(cx).value(), ""));
    }

    #[gpui_kit::test]
    fn whole_note_shortcut_requires_a_ready_preview_and_rereads_the_file(cx: &mut TestAppContext) {
        let (directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", "indexed body")]);
        cx.run_until_parked();
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        cx.simulate_keystrokes(window, "cmd-shift-c down cmd-shift-c enter");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
        window
            .update(cx, |_, window, cx| {
                assert!(!launcher.read(cx).preview_focus.is_focused(window))
            })
            .expect("loading focus unchanged");
        settle_preview(cx);
        fs::write(
            directory.path().join("notes/note.md"),
            "---\ntitle: metadata\n---\nlatest body",
        )
        .expect("external edit");
        cx.simulate_keystrokes(window, "enter cmd-shift-c");
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("latest body".into())
        );
    }

    #[gpui_kit::test]
    fn whole_note_copy_failure_keeps_preview_and_query(cx: &mut TestAppContext) {
        let (directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", "body")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "note");
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down");
        settle_preview(cx);
        fs::remove_file(directory.path().join("notes/note.md")).expect("remove file");
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        cx.simulate_keystrokes(window, "cmd-shift-c");
        cx.run_until_parked();
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.query.read(cx).value(), "note");
            assert!(
                launcher
                    .action_status
                    .as_deref()
                    .is_some_and(|status| status.starts_with("Copy failed:"))
            );
            assert!(matches!(launcher.preview, PreviewState::Ready { .. }));
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
    }

    #[gpui_kit::test]
    fn normal_copy_and_select_all_follow_focus_without_modifying_source(cx: &mut TestAppContext) {
        let source = "# Heading\n\nbody café 🦀";
        let (directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", source)]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "note");
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "cmd-a cmd-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("note".into())
        );
        cx.simulate_keystrokes(window, "down");
        settle_preview(cx);
        cx.simulate_keystrokes(window, "enter cmd-a cmd-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("Heading\n\nbody café 🦀".into())
        );
        cx.update(|cx| assert_eq!(launcher.read(cx).query.read(cx).value(), "note"));
        cx.simulate_keystrokes(window, "escape cmd-a cmd-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("note".into())
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("notes/note.md")).expect("unchanged source"),
            source
        );
        cx.simulate_keystrokes(window, "cmd-shift-c");
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(source.into())
        );
    }

    #[gpui_kit::test]
    fn mouse_selection_copies_only_the_selected_preview_substring(cx: &mut TestAppContext) {
        let (_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", "alpha beta gamma")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down");
        settle_preview(cx);
        let (start, end) = preview_text_points(cx, &launcher, 0, 6, 10);
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::none());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        cx.simulate_keystrokes(window, "cmd-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("beta".into())
        );
        cx.update(|cx| assert_eq!(launcher.read(cx).selected_index, Some(0)));
    }

    #[gpui_kit::test]
    fn mouse_copy_spans_unmounted_virtual_rows(cx: &mut TestAppContext) {
        let body = format!("{}last row", "first row\n\n".repeat(3000));
        let (_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", &body)]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down");
        settle_preview(cx);
        let (start, _) = preview_text_points(cx, &launcher, 0, 2, 2);
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        launcher.update(cx, |launcher, cx| {
            launcher.preview_scroll.scroll_to_end();
            cx.notify();
        });
        cx.run_until_parked();
        assert!(visual.debug_bounds("preview-row-0").is_none());
        let (_, end) = preview_text_points(cx, &launcher, 3000, 0, 8);
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::none());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        cx.simulate_keystrokes(window, "cmd-c");
        let expected = cx.update(|cx| {
            let PreviewState::Ready { document, .. } = &launcher.read(cx).preview else {
                panic!("preview")
            };
            document.selection_text[2..].to_string()
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(expected)
        );
        cx.simulate_keystrokes(window, "cmd-a cmd-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(body)
        );
    }

    #[gpui_kit::test]
    fn drag_autoscroll_stops_after_release_and_new_queries_clear_text_selection(
        cx: &mut TestAppContext,
    ) {
        let body = "paragraph\n\n".repeat(300);
        let (_directory, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("note.md", &body)]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down");
        settle_preview(cx);
        let (start, _) = preview_text_points(cx, &launcher, 0, 0, 0);
        let mut visual = VisualTestContext::from_window(window, cx);
        let viewport = visual.debug_bounds("preview-body").expect("viewport");
        let outside = point(viewport.center().x, viewport.bottom() + px(8.0));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::none());
        cx.executor().advance_clock(Duration::from_millis(120));
        cx.run_until_parked();
        let top = cx.update(|cx| launcher.read(cx).preview_scroll.logical_scroll_top());
        assert!(top.item_ix > 0 || top.offset_in_item > px(0.0));
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
        cx.executor().advance_clock(Duration::from_millis(120));
        cx.run_until_parked();
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            let after = launcher.preview_scroll.logical_scroll_top();
            assert!(!launcher.preview_selection.dragging);
            assert_eq!(after.item_ix, top.item_ix);
            assert_eq!(after.offset_in_item, top.offset_in_item);
        });
        launcher.update(cx, |launcher, cx| {
            launcher.start_search("nothing".into(), cx)
        });
        cx.update(|cx| assert!(launcher.read(cx).preview_selection.range().is_empty()));
    }

    #[gpui_kit::test]
    fn new_note_dialog_defers_creation_and_cancel_preserves_workspace(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "alpha");
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down");
        settle_preview(cx);
        let foreground = cx.update(|cx| gpui_kit::component::Theme::global(cx).foreground);
        cx.simulate_keystrokes(window, "cmd-n");
        cx.run_until_parked();
        cx.update(|cx| {
            let view = launcher.read(cx).creation.as_ref().unwrap().read(cx);
            assert_eq!(view.filename_for_test(cx), "alpha.md");
            assert!(!view.is_busy());
        });
        assert_eq!(fs::read_dir(temp.path().join("notes")).unwrap().count(), 1);
        cx.simulate_keystrokes(window, "escape");
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.creation.is_none());
            assert_eq!(launcher.query.read(cx).value().as_ref(), "alpha");
            assert_eq!(launcher.selected_index, Some(0));
            assert!(launcher.selected_preview_ready());
            assert_eq!(
                gpui_kit::component::Theme::global(cx).foreground,
                foreground
            );
        });
    }

    #[gpui_kit::test]
    fn creates_a_note_from_the_query_and_selects_it_before_indexing(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "bravo");
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "cmd-n enter");
        settle_preview(cx);
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/bravo.md")).unwrap(),
            "# bravo\n"
        );
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.creation.is_none());
            assert_eq!(launcher.query.read(cx).value().as_ref(), "");
            assert!(
                launcher.results[launcher.selected_index.unwrap()]
                    .canonical_path
                    .ends_with("bravo.md")
            );
            assert!(launcher.selected_preview_ready());
        });
        launcher.update(cx, |launcher, cx| launcher.start_search(String::new(), cx));
        settle_preview(cx);
        cx.update(|cx| assert!(launcher.read(cx).selected_preview_ready()));
    }

    #[gpui_kit::test]
    fn duplicate_errors_stay_in_the_dialog_and_can_be_corrected(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "alpha");
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "cmd-n enter");
        cx.run_until_parked();
        cx.update(|cx| {
            let view = launcher.read(cx).creation.as_ref().unwrap().read(cx);
            assert!(view.error_for_test().unwrap().contains("already exists"));
            assert_eq!(view.filename_for_test(cx), "alpha.md");
            assert!(!view.is_busy());
        });
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/alpha.md")).unwrap(),
            "original"
        );
        cx.simulate_keystrokes(window, "cmd-a beta.md enter");
        settle_preview(cx);
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/beta.md")).unwrap(),
            "# beta\n"
        );
        cx.update(|cx| assert!(launcher.read(cx).creation.is_none()));
    }

    #[gpui_kit::test]
    fn outside_destination_errors_preserve_the_draft_and_allow_retry(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) = open_indexed_test_launcher(cx, &[]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "cmd-n");
        let dialog = cx.update(|cx| launcher.read(cx).creation.clone().unwrap());
        dialog.update(cx, |view, _| {
            view.set_destination_for_test(temp.path().to_path_buf())
        });
        cx.simulate_keystrokes(window, "enter");
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(
                dialog
                    .read(cx)
                    .error_for_test()
                    .unwrap()
                    .contains("scan roots")
            )
        });
        assert!(!temp.path().join("Untitled.md").exists());
        let subfolder = temp.path().join("notes/subfolder");
        dialog.update(cx, |view, _| {
            view.set_destination_for_test(subfolder.clone())
        });
        cx.simulate_keystrokes(window, "enter");
        settle_preview(cx);
        assert!(subfolder.join("Untitled.md").is_file());
        cx.simulate_keystrokes(window, "cmd-n");
        cx.update(|cx| {
            assert_eq!(
                launcher
                    .read(cx)
                    .creation
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .destination_for_test(),
                Some(fs::canonicalize(&subfolder).unwrap())
            )
        });
    }

    #[gpui_kit::test]
    fn cancel_from_preview_restores_preview_focus(cx: &mut TestAppContext) {
        let (_temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down");
        settle_preview(cx);
        cx.simulate_keystrokes(window, "enter cmd-n escape");
        window
            .update(cx, |_, window, cx| {
                assert!(launcher.read(cx).preview_focus.is_focused(window))
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn new_note_selection_survives_result_limits_but_not_deletion_or_query_changes(
        cx: &mut TestAppContext,
    ) {
        let names = (0..65)
            .map(|index| format!("a{index:03}.md"))
            .collect::<Vec<_>>();
        let files = names
            .iter()
            .map(|name| (name.as_str(), "existing"))
            .collect::<Vec<_>>();
        let (temp, _, window, launcher) = open_indexed_test_launcher(cx, &files);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "zzz cmd-n enter");
        settle_preview(cx);
        launcher.update(cx, |launcher, cx| launcher.start_search(String::new(), cx));
        settle_preview(cx);
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.results.len(), RESULT_LIMIT);
            assert!(
                launcher.results[launcher.selected_index.unwrap()]
                    .canonical_path
                    .ends_with("zzz.md")
            );
        });
        // Creation pinning is separate from protecting a live editor buffer.
        launcher.update(cx, |launcher, cx| launcher.finish_edit(cx));
        fs::remove_file(temp.path().join("notes/zzz.md")).unwrap();
        launcher.update(cx, |launcher, cx| launcher.start_search(String::new(), cx));
        cx.run_until_parked();
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.created_result.is_none());
            assert!(launcher.selected_index.is_none());
        });
        cx.simulate_keystrokes(window, "cmd-n enter");
        settle_preview(cx);
        cx.simulate_keystrokes(window, "escape aaa");
        cx.run_until_parked();
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.created_result.is_none());
            assert!(launcher.selected_index.is_none());
            assert_eq!(launcher.preview, PreviewState::Empty);
        });
    }

    #[gpui_kit::test]
    fn last_destination_is_persisted_without_changing_scan_configuration(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) = open_indexed_test_launcher(cx, &[]);
        let config_file = temp.path().join("config.toml");
        fs::write(&config_file, "unchanged configuration").unwrap();
        launcher.update(cx, |launcher, _| {
            launcher.config_file = Some(config_file.clone())
        });
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "cmd-n");
        let dialog = cx.update(|cx| launcher.read(cx).creation.clone().unwrap());
        let folder = temp.path().join("notes/team");
        dialog.update(cx, |view, _| view.set_destination_for_test(folder.clone()));
        cx.simulate_keystrokes(window, "enter");
        settle_preview(cx);
        assert_eq!(
            load_last_destination(&config_file.with_file_name("note-creation.json")),
            Some(fs::canonicalize(&folder).unwrap())
        );
        assert_eq!(
            fs::read_to_string(config_file).unwrap(),
            "unchanged configuration"
        );
        launcher.update(cx, |launcher, _| launcher.last_note_destination = None);
        cx.simulate_keystrokes(window, "cmd-n");
        cx.update(|cx| {
            assert_eq!(
                launcher
                    .read(cx)
                    .creation
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .destination_for_test(),
                Some(fs::canonicalize(&folder).unwrap())
            )
        });
    }

    #[gpui_kit::test]
    fn preference_failures_are_warnings_after_successful_creation(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) = open_indexed_test_launcher(cx, &[]);
        fs::create_dir(temp.path().join("note-creation.json")).unwrap();
        launcher.update(cx, |launcher, _| {
            launcher.config_file = Some(temp.path().join("config.toml"))
        });
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "cmd-n enter");
        settle_preview(cx);
        assert!(temp.path().join("notes/Untitled.md").is_file());
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.creation.is_none());
            assert!(launcher.selected_preview_ready());
            assert!(
                launcher
                    .creation_warning
                    .as_ref()
                    .unwrap()
                    .contains("could not be remembered")
            );
        });
    }

    #[gpui_kit::test]
    fn new_note_stays_open_when_the_search_database_is_unavailable(cx: &mut TestAppContext) {
        let (_temp, _, window, launcher) = open_indexed_test_launcher(cx, &[]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "cmd-n enter");
        settle_preview(cx);
        launcher.update(cx, |launcher, cx| {
            launcher.database_path = launcher.database_path.parent().unwrap().to_path_buf();
            launcher.start_search(String::new(), cx);
        });
        settle_preview(cx);
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.search_error.is_some());
            assert!(launcher.selected_preview_ready());
        });
    }

    #[gpui_kit::test]
    fn modal_keys_cannot_copy_or_navigate_the_underlying_workspace(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down");
        settle_preview(cx);
        cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into())));
        cx.simulate_keystrokes(window, "cmd-n down cmd-shift-c cmd-n");
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(launcher.read(cx).creation.is_some());
            assert_eq!(launcher.read(cx).selected_index, Some(0));
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("unchanged")
            );
        });
        assert_eq!(fs::read_dir(temp.path().join("notes")).unwrap().count(), 1);
    }

    #[gpui_kit::test]
    fn caret_contrasts_with_dark_editor_and_white_dialogs_without_changing_text(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            use gpui_kit::component::Theme;
            gpui_kit::init(cx);
            let foreground = Theme::global(cx).foreground;
            init_launcher(cx);
            assert_eq!(Theme::global(cx).foreground, foreground);
            assert_eq!(Theme::global(cx).caret, rgb(CARET_COLOR).into());
        });
        let luminance = |color: u32| {
            let linear = |shift| {
                let value = ((color >> shift) & 255u32) as f64 / 255.;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(16) + 0.7152 * linear(8) + 0.0722 * linear(0)
        };
        for background in [0x27272a, 0xffffff] {
            let caret = luminance(CARET_COLOR);
            let backdrop = luminance(background);
            assert!((caret.max(backdrop) + 0.05) / (caret.min(backdrop) + 0.05) >= 3.);
        }
    }

    #[gpui_kit::test]
    fn editor_escape_saves_shows_preview_and_focuses_search(cx: &mut TestAppContext) {
        let source = "---\ntitle: Alpha\n---\n\n# original\n";
        let (temp, _, window, launcher) = open_indexed_test_launcher(cx, &[("alpha.md", source)]);
        let path = temp.path().join("notes/alpha.md");
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "alpha");
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down cmd-e");
        cx.run_until_parked();
        let editor = cx.update(|cx| launcher.read(cx).editor.clone());
        cx.simulate_keystrokes(window, "escape");
        settle_preview(cx);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        cx.update(|cx| assert_eq!(editor.read(cx).save_count_for_test(), 0));
        cx.simulate_keystrokes(window, "cmd-e");
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.replace_for_test(&source.replace("original", "edited"), window, cx)
                })
            })
            .unwrap();
        cx.simulate_keystrokes(window, "escape");
        settle_preview(cx);
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            source.replace("original", "edited")
        );
        window.update(cx, |_, window, cx| {
            let launcher = launcher.read(cx);
            assert!(editor.read(cx).path().is_none());
            assert_eq!(editor.read(cx).save_count_for_test(), 1);
            assert_eq!(launcher.query.read(cx).value().as_str(), "alpha");
            assert_eq!(launcher.selected_index, Some(0));
            assert!(launcher.query.read(cx).focus_handle(cx).is_focused(window));
            assert!(matches!(&launcher.preview, PreviewState::Ready { document, .. } if document.selection_text.contains("edited")));
        }).unwrap();
        cx.simulate_keystrokes(window, "z");
        cx.run_until_parked();
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.query.read(cx).value().as_str(), "alphaz");
            assert!(editor.read(cx).path().is_none());
            assert!(launcher.selected_index.is_none());
        });
        cx.simulate_keystrokes(window, "escape");
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.query.read(cx).value().is_empty());
            assert!(launcher.selected_index.is_none());
        });
    }

    #[gpui_kit::test]
    fn editor_escape_discards_failed_submission_and_previews_unchanged_source(
        cx: &mut TestAppContext,
    ) {
        let (temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down cmd-e");
        cx.run_until_parked();
        let editor = cx.update(|cx| launcher.read(cx).editor.clone());
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.fail_saves_for_test(true);
                    editor.replace_for_test("lost text", window, cx);
                })
            })
            .unwrap();
        cx.simulate_keystrokes(window, "escape");
        settle_preview(cx);
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/alpha.md")).unwrap(),
            "original"
        );
        window.update(cx, |_, window, cx| {
            let launcher = launcher.read(cx);
            assert!(editor.read(cx).contents_for_test(cx).is_none());
            assert!(editor.read(cx).notice().unwrap().contains("discarded"));
            assert!(launcher.query.read(cx).focus_handle(cx).is_focused(window));
            assert!(matches!(&launcher.preview, PreviewState::Ready { document, .. } if document.selection_text == "original"));
        }).unwrap();
    }

    #[gpui_kit::test]
    fn escape_cancels_a_loading_editor_and_focuses_search(cx: &mut TestAppContext) {
        let (_temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                launcher.update(cx, |launcher, cx| {
                    launcher.select_next(&SelectNext, window, cx);
                    launcher.edit_selected(&EditSelected, window, cx);
                    launcher.dismiss(&Dismiss, window, cx);
                })
            })
            .unwrap();
        settle_preview(cx);
        window
            .update(cx, |_, window, cx| {
                let launcher = launcher.read(cx);
                assert!(launcher.editor.read(cx).path().is_none());
                assert_eq!(launcher.selected_index, Some(0));
                assert!(launcher.query.read(cx).focus_handle(cx).is_focused(window));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn editing_external_notes_preserves_native_text_controls_and_arrow_routing(
        cx: &mut TestAppContext,
    ) {
        let source = "---\ntitle: Alpha\n---\n\n# original\n";
        let (temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", source), ("beta.md", "beta")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down cmd-e");
        cx.run_until_parked();
        let editor = cx.update(|cx| launcher.read(cx).editor.clone());
        cx.update(|cx| {
            assert_eq!(
                editor.read(cx).contents_for_test(cx).as_deref(),
                Some(source)
            )
        });
        cx.simulate_keystrokes(window, "cmd-a cmd-c");
        cx.update(|cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(source)
            )
        });
        cx.simulate_keystrokes(window, "hello enter world up down");
        cx.update(|cx| {
            assert_eq!(launcher.read(cx).selected_index, Some(0));
            assert_eq!(
                editor.read(cx).contents_for_test(cx).as_deref(),
                Some("hello\nworld")
            );
        });
        cx.executor().advance_clock(crate::editor::AUTOSAVE_DELAY);
        cx.run_until_parked();
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/alpha.md")).unwrap(),
            "hello\nworld"
        );
        cx.simulate_keystrokes(window, "escape down");
        settle_preview(cx);
        cx.update(|cx| {
            assert_eq!(launcher.read(cx).selected_index, Some(1));
            assert!(editor.read(cx).path().is_none());
        });
    }

    #[gpui_kit::test]
    fn rapid_preview_navigation_submits_only_the_changed_editor_document(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) = open_indexed_test_launcher(
            cx,
            &[
                ("alpha.md", "alpha"),
                ("beta.md", "beta"),
                ("gamma.md", "gamma"),
            ],
        );
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down cmd-e");
        cx.run_until_parked();
        let editor = cx.update(|cx| launcher.read(cx).editor.clone());
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.replace_for_test("edited alpha", window, cx)
                })
            })
            .unwrap();
        cx.simulate_keystrokes(window, "escape down down up up down up down");
        settle_preview(cx);
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/alpha.md")).unwrap(),
            "edited alpha"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/beta.md")).unwrap(),
            "beta"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/gamma.md")).unwrap(),
            "gamma"
        );
        cx.update(|cx| assert_eq!(editor.read(cx).save_count_for_test(), 1));
    }

    #[gpui_kit::test]
    fn hiding_discards_failed_edits_and_reopening_does_not_restore_them(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down cmd-e");
        cx.run_until_parked();
        let editor = cx.update(|cx| launcher.read(cx).editor.clone());
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.fail_saves_for_test(true);
                    editor.replace_for_test("lost on hide", window, cx);
                })
            })
            .unwrap();
        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.deactivate_window();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(editor.read(cx).path().is_none());
            assert!(
                editor
                    .read(cx)
                    .notice()
                    .unwrap()
                    .contains("Permission denied")
            );
            assert!(editor.read(cx).notice().unwrap().contains("discarded"));
        });
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/alpha.md")).unwrap(),
            "original"
        );
        window
            .update(cx, |_, window, cx| {
                launcher.update(cx, |launcher, cx| launcher.show_launcher(window, cx))
            })
            .unwrap();
        cx.update(|cx| assert!(editor.read(cx).contents_for_test(cx).is_none()));
    }

    #[gpui_kit::test]
    fn window_close_submits_the_last_edit_without_terminating_the_launcher(
        cx: &mut TestAppContext,
    ) {
        let (temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down cmd-e");
        cx.run_until_parked();
        let editor = cx.update(|cx| launcher.read(cx).editor.clone());
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.replace_for_test("saved on close", window, cx)
                })
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(window, cx);
        assert!(!visual.simulate_close());
        cx.run_until_parked();
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/alpha.md")).unwrap(),
            "saved on close"
        );
        cx.update(|cx| assert!(editor.read(cx).contents_for_test(cx).is_none()));
    }

    #[gpui_kit::test]
    fn new_notes_open_in_the_editor_with_the_cursor_after_the_heading(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) = open_indexed_test_launcher(cx, &[]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "cmd-n enter");
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "content cmd-s");
        cx.run_until_parked();
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/Untitled.md")).unwrap(),
            "# Untitled\ncontent"
        );
        cx.update(|cx| assert!(launcher.read(cx).editor.read(cx).path().is_some()));
    }

    #[gpui_kit::test]
    fn canceling_new_note_restores_the_current_editor_and_its_buffer(cx: &mut TestAppContext) {
        let (temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down cmd-e");
        cx.run_until_parked();
        let editor = cx.update(|cx| launcher.read(cx).editor.clone());
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.replace_for_test("current draft", window, cx)
                })
            })
            .unwrap();
        cx.simulate_keystrokes(window, "cmd-n escape");
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                assert!(editor.read(cx).is_focused(window, cx));
                assert_eq!(
                    editor.read(cx).contents_for_test(cx).as_deref(),
                    Some("current draft")
                );
            })
            .unwrap();
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/alpha.md")).unwrap(),
            "original"
        );
    }

    #[gpui_kit::test]
    fn index_refreshes_do_not_replace_a_live_editor_and_external_changes_conflict(
        cx: &mut TestAppContext,
    ) {
        let (temp, database_path, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "searchterm")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "searchterm");
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down cmd-e");
        cx.run_until_parked();
        let editor = cx.update(|cx| launcher.read(cx).editor.clone());
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.replace_for_test("local draft", window, cx)
                })
            })
            .unwrap();
        fs::write(temp.path().join("notes/alpha.md"), "external revision").unwrap();
        MarkdownIndexer::new(
            &ScanConfig::default_for_home(temp.path().join("notes")),
            &mut IndexDatabase::open(&database_path).unwrap(),
        )
        .run()
        .unwrap();
        launcher.update(cx, |launcher, cx| {
            launcher.start_search("searchterm".into(), cx)
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                editor.read(cx).contents_for_test(cx).as_deref(),
                Some("local draft")
            )
        });
        launcher.update(cx, |launcher, cx| launcher.finish_edit(cx));
        cx.run_until_parked();
        assert_eq!(
            fs::read_to_string(temp.path().join("notes/alpha.md")).unwrap(),
            "external revision"
        );
        cx.update(|cx| assert!(editor.read(cx).notice().unwrap().contains("Conflict")));
    }

    #[gpui_kit::test]
    fn deleting_an_edited_file_keeps_the_active_buffer_until_explicit_departure(
        cx: &mut TestAppContext,
    ) {
        let (temp, database_path, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "original")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down cmd-e");
        cx.run_until_parked();
        let editor = cx.update(|cx| launcher.read(cx).editor.clone());
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.replace_for_test("current draft", window, cx)
                })
            })
            .unwrap();
        let path = temp.path().join("notes/alpha.md");
        fs::remove_file(&path).unwrap();
        MarkdownIndexer::new(
            &ScanConfig::default_for_home(temp.path().join("notes")),
            &mut IndexDatabase::open(&database_path).unwrap(),
        )
        .run()
        .unwrap();
        launcher.update(cx, |launcher, cx| launcher.start_search(String::new(), cx));
        cx.executor().advance_clock(crate::editor::AUTOSAVE_DELAY);
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                editor.read(cx).contents_for_test(cx).as_deref(),
                Some("current draft")
            );
            assert!(editor.read(cx).notice().unwrap().contains("unavailable"));
        });
        launcher.update(cx, |launcher, cx| launcher.finish_edit(cx));
        cx.run_until_parked();
        assert!(!path.exists());
        cx.update(|cx| assert!(editor.read(cx).contents_for_test(cx).is_none()));
    }

    #[gpui_kit::test]
    fn switching_before_editor_loading_finishes_rejects_the_stale_open(cx: &mut TestAppContext) {
        let (_temp, _, window, launcher) =
            open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                launcher.update(cx, |launcher, cx| {
                    launcher.select_next(&SelectNext, window, cx);
                    launcher.edit_selected(&EditSelected, window, cx);
                    launcher.select_next(&SelectNext, window, cx);
                })
            })
            .unwrap();
        settle_preview(cx);
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.selected_index, Some(1));
            assert!(launcher.editor.read(cx).path().is_none());
        });
    }

    #[gpui_kit::test]
    fn whole_note_copy_is_disabled_while_editing_until_draft_copy_is_implemented(
        cx: &mut TestAppContext,
    ) {
        let (_temp, _, window, launcher) = open_indexed_test_launcher(cx, &[("alpha.md", "alpha")]);
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "down");
        settle_preview(cx);
        cx.simulate_keystrokes(window, "cmd-e");
        cx.run_until_parked();
        cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into())));
        cx.simulate_keystrokes(window, "cmd-shift-c");
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("unchanged")
            );
            assert!(launcher.read(cx).editor.read(cx).path().is_some());
        });
    }

    fn preview_text_points(
        cx: &mut TestAppContext,
        launcher: &Entity<Launcher>,
        row: usize,
        start: usize,
        end: usize,
    ) -> (
        gpui_kit::Point<gpui_kit::Pixels>,
        gpui_kit::Point<gpui_kit::Pixels>,
    ) {
        cx.update(|cx| {
            let launcher = launcher.read(cx);
            let layouts = launcher.preview_layouts.borrow();
            let layout = layouts.get(&row).expect("mounted row");
            let middle = point(px(0.0), layout.line_height() / 2.0);
            (
                layout.position_for_index(start).expect("start") + middle,
                layout.position_for_index(end).expect("end") + middle,
            )
        })
    }

    pub(super) fn settle_preview(cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.executor().advance_clock(PREVIEW_DEBOUNCE);
        cx.run_until_parked();
    }

    pub(super) fn open_indexed_test_launcher(
        cx: &mut TestAppContext,
        files: &[(&str, &str)],
    ) -> (
        tempfile::TempDir,
        PathBuf,
        AnyWindowHandle,
        Entity<Launcher>,
    ) {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let notes = temporary_directory.path().join("notes");
        fs::create_dir(&notes).expect("notes");
        for (name, body) in files {
            let path = notes.join(name);
            fs::create_dir_all(path.parent().expect("parent")).expect("parent directories");
            fs::write(path, body).expect("note");
        }
        let database_path = temporary_directory.path().join("index.sqlite3");
        let mut database = IndexDatabase::open(&database_path).expect("database");
        MarkdownIndexer::new(&ScanConfig::default_for_home(&notes), &mut database)
            .run()
            .expect("index");
        drop(database);
        let (window, launcher) = open_test_launcher(cx, database_path.clone());
        launcher.update(cx, |launcher, _| {
            launcher.scan_config = Some(ScanConfig::default_for_home(&notes))
        });
        (temporary_directory, database_path, window, launcher)
    }

    fn open_test_launcher(
        cx: &mut TestAppContext,
        database_path: PathBuf,
    ) -> (AnyWindowHandle, Entity<Launcher>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            init_launcher(cx);
            let bounds = Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(1040.0), px(640.0)),
            };
            let (window, launcher) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    cx.new(|cx| {
                        Launcher::new(
                            database_path,
                            1024 * 1024,
                            LauncherServices::default(),
                            None,
                            window,
                            cx,
                        )
                    })
                },
            )
            .expect("open launcher");
            (window, launcher)
        })
    }
}
