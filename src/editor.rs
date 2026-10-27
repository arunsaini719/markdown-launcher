use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, TestSupportExt as _, Window,
    component::input::{InputEvent, Textarea, TextareaState},
    div,
    prelude::FluentBuilder as _,
    rgb,
};

use crate::{
    config::ScanConfig,
    editing::{self, EditSource, FileVersion, SaveOutcome},
    workspace_ui::{document_header, primary_button},
};

#[cfg(test)]
mod tests;

pub(crate) const AUTOSAVE_DELAY: Duration = Duration::from_millis(500);
pub(crate) const EDITOR_KEY_CONTEXT: &str = "MarkdownEditing";
type SaveFn = fn(&ScanConfig, &Path, &FileVersion, &str) -> Result<SaveOutcome, String>;

pub(crate) enum EditorEvent {
    Changed,
    Saved(PathBuf),
    QuitReady,
}

struct Session {
    epoch: u64,
    path: PathBuf,
    config: ScanConfig,
    text: Entity<TextareaState>,
    baseline: FileVersion,
    revision: u64,
    saved_revision: u64,
    error: Option<String>,
    _subscription: Subscription,
    _focus_subscription: Subscription,
    _blur_subscription: Subscription,
}

struct Snapshot {
    epoch: u64,
    revision: u64,
    path: PathBuf,
    config: ScanConfig,
    expected: FileVersion,
    text: String,
}

#[derive(Clone)]
struct WriteIdentity {
    epoch: u64,
    revision: u64,
    path: PathBuf,
    snapshot: Arc<Snapshot>,
}

type SaveGate = Arc<Mutex<Option<(u64, u64, FileVersion)>>>;

pub(crate) struct EditorWorkspace {
    session: Option<Session>,
    opening: Option<(ScanConfig, PathBuf)>,
    generation: u64,
    loading: bool,
    inflight: Option<WriteIdentity>,
    pending: VecDeque<Arc<Snapshot>>,
    save_gate: SaveGate,
    notice: Option<String>,
    detached_failure: Option<(PathBuf, String)>,
    quitting: bool,
    save_fn: SaveFn,
    _load_task: Task<()>,
    _debounce_task: Task<()>,
    _save_task: Task<()>,
    #[cfg(test)]
    started_saves: usize,
}

impl EventEmitter<EditorEvent> for EditorWorkspace {}

impl EditorWorkspace {
    pub(crate) fn new() -> Self {
        Self {
            session: None,
            opening: None,
            generation: 0,
            loading: false,
            inflight: None,
            pending: VecDeque::new(),
            save_gate: Arc::default(),
            notice: None,
            detached_failure: None,
            quitting: false,
            save_fn: editing::save,
            _load_task: Task::ready(()),
            _debounce_task: Task::ready(()),
            _save_task: Task::ready(()),
            #[cfg(test)]
            started_saves: 0,
        }
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        self.session
            .as_ref()
            .map(|session| session.path.as_path())
            .or_else(|| self.opening.as_ref().map(|(_, path)| path.as_path()))
    }

    pub(crate) fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.text.read(cx).focus_handle(cx).is_focused(window))
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            session.text.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.emit(EditorEvent::Changed);
    }

    pub(crate) fn has_draft(&self) -> bool {
        self.session.is_some()
    }

    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_deref().or_else(|| {
            self.detached_failure
                .as_ref()
                .map(|(_, reason)| reason.as_str())
        })
    }

    pub(crate) fn open(
        &mut self,
        config: ScanConfig,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quitting {
            return;
        }
        if self.path() == Some(path.as_path()) {
            self.focus(window, cx);
            return;
        }
        self.leave(cx);
        self.opening = Some((config, path));
        self.start_open(window, cx);
    }

    fn start_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some((config, path)) = self.opening.clone() else {
            return;
        };
        // Only reopening the same file waits for its submitted writes. Other
        // documents remain editable while detached saves finish.
        if self.inflight.as_ref().is_some_and(|job| job.path == path)
            || self.pending.iter().any(|job| job.path == path)
        {
            return;
        }
        self.loading = true;
        let generation = self.generation;
        self._load_task = cx.spawn_in(window, async move |this, cx| {
            let read_path = path.clone();
            let read_config = config.clone();
            let result = cx
                .background_spawn(async move { editing::load(&read_config, &read_path) })
                .await;
            _ = this.update_in(cx, |view, window, cx| {
                if view.generation != generation
                    || view
                        .opening
                        .as_ref()
                        .is_none_or(|(_, current)| *current != path)
                {
                    return;
                }
                view.opening = None;
                view.loading = false;
                match result {
                    Ok(EditSource { text, version }) => {
                        let end = text.len();
                        let input = cx.new(|cx| TextareaState::new(window, cx).default_value(text));
                        input.update(cx, |input, cx| input.set_selected_range(end..end, cx));
                        let subscription = cx.subscribe(&input, |view, _, event, cx| {
                            if matches!(event, InputEvent::Change) {
                                view.changed(cx);
                            }
                        });
                        let handle = input.read(cx).focus_handle(cx);
                        let focus_subscription =
                            cx.on_focus(&handle, window, |view, _, cx| view.changed_view(cx));
                        let blur_subscription =
                            cx.on_blur(&handle, window, |view, _, cx| view.changed_view(cx));
                        view.session = Some(Session {
                            epoch: generation,
                            path,
                            config,
                            text: input,
                            baseline: version,
                            revision: 0,
                            saved_revision: 0,
                            error: None,
                            _subscription: subscription,
                            _focus_subscription: focus_subscription,
                            _blur_subscription: blur_subscription,
                        });
                        view.focus(window, cx);
                    }
                    Err(error) => {
                        view.notice = Some(format!("Could not edit {}: {error}", path.display()))
                    }
                }
                view.changed_view(cx);
            });
        });
        self.changed_view(cx);
    }

    fn changed_view(&self, cx: &mut Context<Self>) {
        cx.notify();
        cx.emit(EditorEvent::Changed);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        session.revision = session.revision.wrapping_add(1);
        session.error = None;
        self._debounce_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            _ = this.update(cx, |view, cx| view.request_save(cx));
        });
        self.changed_view(cx);
    }

    pub(crate) fn request_save(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.revision == session.saved_revision
            || self
                .inflight
                .as_ref()
                .is_some_and(|job| job.epoch == session.epoch && job.revision == session.revision)
            || self
                .pending
                .iter()
                .any(|job| job.epoch == session.epoch && job.revision == session.revision)
        {
            return;
        }
        session.error = None;
        let text = session.text.read(cx).value().to_string();
        if self.inflight.is_none() && session.baseline.matches_text(&text) {
            session.saved_revision = session.revision;
            self.changed_view(cx);
            return;
        }
        let snapshot = Snapshot {
            epoch: session.epoch,
            revision: session.revision,
            path: session.path.clone(),
            config: session.config.clone(),
            expected: session.baseline.clone(),
            text,
        };
        self.enqueue_snapshot(Arc::new(snapshot));
        self.start_save(cx);
    }

    fn enqueue_snapshot(&mut self, snapshot: Arc<Snapshot>) {
        if let Some(pending) = self
            .pending
            .iter_mut()
            .find(|pending| pending.epoch == snapshot.epoch)
        {
            if pending.revision <= snapshot.revision {
                *pending = snapshot;
            }
        } else {
            self.pending.push_back(snapshot);
        }
    }

    fn start_save(&mut self, cx: &mut Context<Self>) {
        if self.inflight.is_some() {
            return;
        }
        let Some(snapshot) = self.pending.pop_front() else {
            return;
        };
        if let Some(session) = &mut self.session
            && session.epoch == snapshot.epoch
        {
            session.error = None;
        }
        let identity = WriteIdentity {
            epoch: snapshot.epoch,
            revision: snapshot.revision,
            path: snapshot.path.clone(),
            snapshot: Arc::clone(&snapshot),
        };
        self.inflight = Some(identity.clone());
        #[cfg(test)]
        {
            self.started_saves += 1;
        }
        let save = self.save_fn;
        let gate = Arc::clone(&self.save_gate);
        self._save_task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { Self::write_snapshot(&snapshot, &gate, save) })
                .await;
            _ = this.update(cx, |view, cx| view.complete_save(identity, result, cx));
        });
        self.changed_view(cx);
    }

    fn complete_save(
        &mut self,
        identity: WriteIdentity,
        result: Result<SaveOutcome, String>,
        cx: &mut Context<Self>,
    ) {
        self.inflight = None;
        match result {
            Ok(outcome) => {
                if let Some(session) = &mut self.session
                    && session.epoch == identity.epoch
                {
                    session.baseline = outcome.version.clone();
                    session.saved_revision = identity.revision;
                    session.error = None;
                }
                if self
                    .detached_failure
                    .as_ref()
                    .is_some_and(|(path, _)| *path == identity.path)
                {
                    self.detached_failure = None;
                }
                self.notice = outcome.warning;
                cx.emit(EditorEvent::Saved(identity.path));
            }
            Err(error) => {
                let retained = self
                    .session
                    .as_mut()
                    .filter(|session| session.epoch == identity.epoch);
                if let Some(session) = retained {
                    session.error = Some(error.clone());
                }
                let discarded = self
                    .session
                    .as_ref()
                    .is_none_or(|session| session.epoch != identity.epoch);
                self.notice = Some(format!(
                    "Save failed for {}: {error}{}",
                    identity.path.display(),
                    if discarded {
                        ". Unsaved changes discarded."
                    } else {
                        ""
                    }
                ));
                if discarded {
                    self.detached_failure = self
                        .notice
                        .as_ref()
                        .map(|reason| (identity.path.clone(), reason.clone()));
                }
            }
        }
        self.start_save(cx);
        if self.quitting && self.inflight.is_none() && self.pending.is_empty() {
            cx.emit(EditorEvent::QuitReady);
        }
        self.changed_view(cx);
    }

    fn write_snapshot(
        snapshot: &Snapshot,
        gate: &SaveGate,
        save: SaveFn,
    ) -> Result<SaveOutcome, String> {
        let mut previous = gate
            .lock()
            .map_err(|_| "The save worker failed".to_string())?;
        if let Some((epoch, revision, version)) = previous.as_ref()
            && *epoch == snapshot.epoch
            && *revision >= snapshot.revision
        {
            return Ok(SaveOutcome {
                version: version.clone(),
                warning: None,
            });
        }
        let expected = previous
            .as_ref()
            .filter(|(epoch, _, _)| *epoch == snapshot.epoch)
            .map_or(&snapshot.expected, |(_, _, version)| version);
        let result = save(&snapshot.config, &snapshot.path, expected, &snapshot.text)?;
        *previous = Some((snapshot.epoch, snapshot.revision, result.version.clone()));
        Ok(result)
    }

    pub(crate) fn prepare_shutdown(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let latest = self
            .session
            .as_ref()
            .filter(|session| session.revision != session.saved_revision)
            .map(|session| {
                Arc::new(Snapshot {
                    epoch: session.epoch,
                    revision: session.revision,
                    path: session.path.clone(),
                    config: session.config.clone(),
                    expected: session.baseline.clone(),
                    text: session.text.read(cx).value().to_string(),
                })
            });
        if let Some(snapshot) = latest
            && !self
                .inflight
                .as_ref()
                .is_some_and(|job| job.epoch == snapshot.epoch && job.revision == snapshot.revision)
        {
            self.enqueue_snapshot(snapshot);
        }
        let mut submitted = std::mem::take(&mut self.pending);
        if let Some(identity) = self.inflight.take() {
            submitted.push_front(identity.snapshot);
        }
        self.session = None;
        self.opening = None;
        self.loading = false;
        self.quitting = true;
        self.generation = self.generation.wrapping_add(1);
        self._load_task = Task::ready(());
        self._debounce_task = Task::ready(());
        self._save_task = Task::ready(());
        let gate = Arc::clone(&self.save_gate);
        let save = self.save_fn;
        // App shutdown waits with its app borrow held. This final attempt must
        // never call back into GPUI, including when another write is finishing.
        cx.background_spawn(async move {
            for snapshot in submitted {
                let _ = Self::write_snapshot(&snapshot, &gate, save);
            }
        })
    }

    pub(crate) fn resume_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening.is_some() {
            self.start_open(window, cx);
        }
    }

    pub(crate) fn leave(&mut self, cx: &mut Context<Self>) {
        self.request_save(cx);
        self._debounce_task = Task::ready(());
        self._load_task = Task::ready(());
        self.generation = self.generation.wrapping_add(1);
        self.opening = None;
        self.loading = false;
        self.session = None;
        self.changed_view(cx);
    }

    pub(crate) fn quit(&mut self, cx: &mut Context<Self>) {
        self.leave(cx);
        self.quitting = true;
        if self.inflight.is_none() && self.pending.is_empty() {
            cx.emit(EditorEvent::QuitReady);
        }
    }
}

#[cfg(test)]
impl EditorWorkspace {
    pub(crate) fn replace_for_test(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session
            .as_ref()
            .expect("editor session")
            .text
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.changed(cx);
    }

    pub(crate) fn contents_for_test(&self, cx: &App) -> Option<String> {
        self.session
            .as_ref()
            .map(|session| session.text.read(cx).value().to_string())
    }

    pub(crate) fn save_count_for_test(&self) -> usize {
        self.started_saves
    }

    pub(crate) fn fail_saves_for_test(&mut self, fail: bool) {
        self.save_fn = if fail {
            |_, _, _, _| Err("Permission denied (simulated)".into())
        } else {
            editing::save
        };
    }
}

impl Render for EditorWorkspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = &self.session else {
            return div()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .text_color(rgb(0xf4f4f5))
                .when_some(self.opening.as_ref(), |view, (_, path)| {
                    view.child(document_header("Opening editor", path, "Escape: search"))
                })
                .child(if !self.loading && self.opening.is_some() {
                    "Waiting for this file's previous save..."
                } else {
                    "Loading complete Markdown source..."
                });
        };
        let status = if session.error.is_some() {
            "Save failed"
        } else if self
            .inflight
            .as_ref()
            .is_some_and(|job| job.epoch == session.epoch)
            || self.pending.iter().any(|job| job.epoch == session.epoch)
        {
            "Saving..."
        } else if session.revision != session.saved_revision {
            "Unsaved"
        } else {
            "Saved"
        };
        let failed = session.error.is_some();
        let input = session.text.clone();
        let status_color = match status {
            "Save failed" => rgb(0xfca5a5),
            "Unsaved" => rgb(0xfde68a),
            "Saving..." => rgb(0x93c5fd),
            _ => rgb(0x86efac),
        };
        div()
            .size_full()
            .debug_selector(|| "document-editor".into())
            .flex()
            .flex_col()
            .min_h_0()
            .gap_2()
            .p_3()
            .bg(rgb(0x27272a))
            .text_color(rgb(0xf4f4f5))
            .child(document_header(
                "Editing",
                &session.path,
                "Cmd/Ctrl+S: save - Escape: search",
            ))
            .child(
                div()
                    .id("editor-save-status")
                    .role(gpui_kit::accesskit::Role::Status)
                    .aria_label(status)
                    .debug_selector(|| "editor-save-status".into())
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(status_color)
                    .child(status)
                    .test_support(),
            )
            .child(
                Textarea::new(&input)
                    .aria_label("Markdown source editor")
                    .appearance(false)
                    .bordered(false)
                    .bg(rgb(0x27272a))
                    .text_color(rgb(0xf4f4f5))
                    .h_full()
                    .flex_1()
                    .min_h_0(),
            )
            .when(failed, |view| {
                view.child(
                    div()
                        .id("editor-save-error")
                        .debug_selector(|| "editor-save-error".into())
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .gap_2()
                        .p_2()
                        .max_h(gpui_kit::px(128.0))
                        .rounded_md()
                        .bg(rgb(0x3b2025))
                        .border_1()
                        .border_color(rgb(0x7f1d1d))
                        .child(
                            div()
                                .id("editor-error-detail")
                                .max_h(gpui_kit::px(72.0))
                                .overflow_y_scroll()
                                .text_sm()
                                .text_color(rgb(0xfca5a5))
                                .child(session.error.clone().unwrap_or_default()),
                        )
                        .child(
                            primary_button("retry-editor-save", cx)
                                .flex_shrink_0()
                                .label("Retry Save")
                                .tooltip(
                                    "Retry saving the latest active editor buffer (Cmd/Ctrl+S)",
                                )
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.request_save(cx);
                                    view.focus(window, cx);
                                })),
                        ),
                )
            })
    }
}
