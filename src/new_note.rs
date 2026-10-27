use std::path::PathBuf;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Hsla,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render, Styled as _,
    Subscription, Task, Window,
    component::{
        Disableable as _, Theme,
        button::{Button, ButtonVariants as _},
        input::{Input, InputEvent, InputState},
    },
    div,
    prelude::FluentBuilder as _,
    px, rgb,
};

use crate::{
    config::ScanConfig,
    launcher::QuitApplication,
    notes::{
        CreatedNote, create_note, save_last_destination, suggested_destination, suggested_filename,
    },
};

const KEY_CONTEXT: &str = "MarkdownNewNote";
gpui_kit::actions!(markdown_new_note, [ConfirmNote, CancelNote]);

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", ConfirmNote, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", CancelNote, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-q", QuitApplication, Some(KEY_CONTEXT)),
    ]);
}

pub(crate) enum NewNoteEvent {
    Cancelled,
    Created(CreatedNote),
}

pub(crate) struct NewNoteView {
    config: ScanConfig,
    preference_file: Option<PathBuf>,
    filename: Entity<InputState>,
    destination: Option<PathBuf>,
    focus: FocusHandle,
    original_foreground: Hsla,
    error: Option<String>,
    busy: bool,
    choosing_destination: bool,
    _picker_task: Task<()>,
    _task: Task<()>,
    _subscription: Subscription,
}

impl EventEmitter<NewNoteEvent> for NewNoteView {}

impl NewNoteView {
    pub(crate) fn new(
        config: ScanConfig,
        query: &str,
        remembered: Option<&std::path::Path>,
        preference_file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let original_foreground = Theme::global(cx).foreground;
        Theme::update(cx, |theme| theme.colors.foreground = rgb(0x000000).into());
        let filename = cx.new(|cx| {
            InputState::new(window, cx).default_value(suggested_filename(query, &config))
        });
        filename.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let subscription = cx.subscribe(&filename, |view, _, event, cx| {
            if matches!(event, InputEvent::Change) && !view.busy {
                view.error = None;
                cx.notify();
            }
        });
        let destination = suggested_destination(&config, remembered);
        Self {
            config,
            preference_file,
            filename,
            destination,
            focus: cx.focus_handle(),
            original_foreground,
            error: None,
            busy: false,
            choosing_destination: false,
            _picker_task: Task::ready(()),
            _task: Task::ready(()),
            _subscription: subscription,
        }
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.busy || self.choosing_destination
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.filename
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub(crate) fn restore_theme(&self, cx: &mut App) {
        Theme::update(cx, |theme| {
            theme.colors.foreground = self.original_foreground
        });
    }

    fn choose_destination(
        &mut self,
        _: &gpui_kit::ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let mut picker = rfd::AsyncFileDialog::new()
                .set_parent(_window)
                .set_title("Choose a destination inside a Markdown scan root");
            if let Some(directory) = &self.destination {
                picker = picker.set_directory(directory);
            }
            self.choosing_destination = true;
            cx.notify();
            self._picker_task = cx.spawn_in(_window, async move |this, cx| {
                let chosen = picker.pick_folder().await;
                _ = this.update_in(cx, |view, window, cx| {
                    view.choosing_destination = false;
                    if let Some(directory) = chosen {
                        let directory = directory.path().to_path_buf();
                        view.error = crate::notes::check_destination(
                            &view.config,
                            &directory,
                            view.filename.read(cx).value().as_ref(),
                        )
                        .err();
                        view.destination = Some(directory);
                    }
                    view.focus(window, cx);
                    cx.notify();
                });
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.error = Some("The folder picker is currently available only on macOS".into());
            cx.notify();
        }
    }

    fn cancel(&mut self, _: &CancelNote, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_busy() {
            cx.emit(NewNoteEvent::Cancelled);
        }
    }

    fn confirm(&mut self, _: &ConfirmNote, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let Some(directory) = self.destination.clone() else {
            self.error = Some("Choose a destination inside an active scan root".into());
            cx.notify();
            return;
        };
        let filename = self.filename.read(cx).value().to_string();
        let config = self.config.clone();
        let preference_file = self.preference_file.clone();
        self.busy = true;
        self.error = None;
        cx.notify();
        self._task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut note = create_note(&config, &directory, &filename)?;
                    if let Some(file) = preference_file
                        && let Err(error) = save_last_destination(
                            &file,
                            note.path.parent().expect("a note has a parent directory"),
                        )
                    {
                        let warning = format!(
                            "Note created, but its destination could not be remembered: {error}"
                        );
                        note.warning =
                            Some(note.warning.map_or(warning.clone(), |existing| {
                                format!("{existing}; {warning}")
                            }));
                    }
                    Ok::<_, String>(note)
                })
                .await;
            _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(note) => cx.emit(NewNoteEvent::Created(note)),
                    Err(error) => {
                        view.error = Some(error);
                        cx.notify();
                    }
                }
            });
        });
    }

    #[cfg(test)]
    pub(crate) fn filename_for_test(&self, cx: &App) -> String {
        self.filename.read(cx).value().to_string()
    }

    #[cfg(test)]
    pub(crate) fn set_destination_for_test(&mut self, path: PathBuf) {
        self.destination = Some(path);
    }

    #[cfg(test)]
    pub(crate) fn destination_for_test(&self) -> Option<PathBuf> {
        self.destination.clone()
    }

    #[cfg(test)]
    pub(crate) fn error_for_test(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

impl Render for NewNoteView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_busy();
        let destination = self.destination.as_ref().map_or_else(
            || "Choose a folder".into(),
            |path| path.to_string_lossy().into_owned(),
        );
        let panel = div()
            .id("new-note-dialog")
            .w(px(580.0))
            .max_w_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .rounded_lg()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x000000))
            .child(div().text_lg().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("New Note"))
            .child(div().text_sm().child("Filename (including extension)"))
            .child(Input::new(&self.filename)
                .id("new-note-filename")
                .aria_label("New note filename")
                .bg(rgb(0xffffff))
                .text_color(rgb(0x000000))
                .disabled(busy))
            .child(div().text_sm().child("Destination"))
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().text_sm().child(destination))
                .child(Button::new("choose-note-destination")
                    .label("Choose...")
                    .disabled(busy)
                    .on_click(cx.listener(Self::choose_destination))))
            .child(div().text_xs().text_color(rgb(0x52525b)).child(
                "Choose a folder inside an active scan root. Exclusions and symlink settings apply. Missing subfolders are created only when you confirm."
            ))
            .when_some(self.error.clone(), |view, error| {
                view.child(div().id("new-note-error").text_sm().text_color(rgb(0xb91c1c)).child(error))
            })
            .when(self.busy, |view| {
                view.child(div().text_sm().child("Creating note... Please wait."))
            })
            .child(div().flex().justify_end().gap_2()
                .child(Button::new("cancel-new-note")
                    .label("Cancel")
                    .disabled(busy)
                    .on_click(cx.listener(|view, _, window, cx| view.cancel(&CancelNote, window, cx))))
                .child(Button::new("confirm-new-note")
                    .primary()
                    .label("Create Note")
                    .disabled(busy)
                    .on_click(cx.listener(|view, _, window, cx| view.confirm(&ConfirmNote, window, cx)))));
        div()
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x18181b))
            .p_6()
            .child(panel)
    }
}
