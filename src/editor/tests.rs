use super::*;
use gpui_kit::{AnyWindowHandle, TestAppContext, WindowOptions, test::TestWindowExt as _};
use std::{cell::Cell, fs, rc::Rc};

const ORIGINAL: &str = "\u{feff}---\r\ntitle: Original\r\n---\r\n\r\n# Original\r\n";

fn open(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    PathBuf,
    AnyWindowHandle,
    Entity<EditorWorkspace>,
) {
    cx.update(gpui_kit::init);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("external.md");
    fs::write(&path, ORIGINAL).unwrap();
    let path = fs::canonicalize(path).unwrap();
    let config = ScanConfig::default_for_home(directory.path());
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|_| EditorWorkspace::new())
            })
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| view.open(config, path.clone(), window, cx))
        })
        .unwrap();
    cx.run_until_parked();
    (directory, path, window, view)
}

fn replace(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<EditorWorkspace>,
    text: &str,
) {
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| view.replace_for_test(text, window, cx))
        })
        .unwrap();
}

#[gpui_kit::test]
fn loading_and_leaving_unchanged_source_never_writes(cx: &mut TestAppContext) {
    let (_directory, path, _, view) = open(cx);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    cx.update(|cx| {
        assert_eq!(
            view.read(cx).contents_for_test(cx).as_deref(),
            Some(ORIGINAL)
        )
    });
    view.update(cx, |view, cx| view.leave(cx));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&path).unwrap(), ORIGINAL);
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
    cx.update(|cx| assert_eq!(view.read(cx).started_saves, 0));
}

#[gpui_kit::test]
fn returning_to_saved_content_before_autosave_does_not_write(cx: &mut TestAppContext) {
    let (_directory, path, window, view) = open(cx);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    replace(cx, window, &view, "temporary edit");
    replace(cx, window, &view, ORIGINAL);
    view.update(cx, |view, cx| view.leave(cx));
    cx.run_until_parked();
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
    cx.update(|cx| assert_eq!(view.read(cx).started_saves, 0));
}

#[gpui_kit::test]
fn returning_to_the_old_baseline_while_a_write_is_in_flight_restores_it(cx: &mut TestAppContext) {
    let (_directory, path, window, view) = open(cx);
    replace(cx, window, &view, "intermediate");
    view.update(cx, |view, cx| view.request_save(cx));
    replace(cx, window, &view, ORIGINAL);
    view.update(cx, |view, cx| view.leave(cx));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), ORIGINAL);
}

#[gpui_kit::test]
fn autosave_debounces_to_the_latest_revision(cx: &mut TestAppContext) {
    let (_directory, path, window, view) = open(cx);
    replace(cx, window, &view, "first");
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    replace(cx, window, &view, "latest");
    cx.executor().advance_clock(Duration::from_millis(499));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&path).unwrap(), ORIGINAL);
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), "latest");
    cx.update(|cx| {
        let view = view.read(cx);
        let session = view.session.as_ref().unwrap();
        assert_eq!(session.revision, session.saved_revision);
        assert_eq!(view.started_saves, 1);
    });
}

#[gpui_kit::test]
fn saves_are_serialized_and_only_the_latest_pending_snapshot_survives(cx: &mut TestAppContext) {
    let (_directory, path, window, view) = open(cx);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.replace_for_test("first", window, cx);
                view.request_save(cx);
                view.replace_for_test("second", window, cx);
                view.request_save(cx);
                view.replace_for_test("third", window, cx);
                view.request_save(cx);
                assert_eq!(view.pending.back().unwrap().text, "third");
                assert_eq!(view.started_saves, 1);
                view.leave(cx);
                assert!(view.session.is_none());
            })
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), "third");
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.started_saves, 2);
        assert!(view.pending.is_empty());
        assert!(view.inflight.is_none());
        assert!(view.session.is_none());
    });
}

#[gpui_kit::test]
fn failed_autosave_keeps_only_the_active_buffer_and_retry_uses_latest_text(
    cx: &mut TestAppContext,
) {
    let (_directory, path, window, view) = open(cx);
    view.update(cx, |view, _| view.fail_saves_for_test(true));
    replace(cx, window, &view, "first draft");
    view.update(cx, |view, cx| view.request_save(cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.contents_for_test(cx).as_deref(), Some("first draft"));
        assert!(
            view.session
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("Permission denied")
        );
    });
    replace(cx, window, &view, "latest draft");
    view.update(cx, |view, _| view.fail_saves_for_test(false));
    view.update(cx, |view, cx| view.request_save(cx));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), "latest draft");
    cx.update(|cx| assert!(view.read(cx).session.as_ref().unwrap().error.is_none()));
}

#[gpui_kit::test]
fn retry_button_retries_the_active_revision(cx: &mut TestAppContext) {
    let (_directory, path, window, view) = open(cx);
    view.update(cx, |view, _| view.fail_saves_for_test(true));
    replace(cx, window, &view, "retry me");
    view.update(cx, |view, cx| view.request_save(cx));
    cx.run_until_parked();
    view.update(cx, |view, _| view.fail_saves_for_test(false));
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("retry-editor-save", cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), "retry me");
}

#[gpui_kit::test]
fn failed_departure_discards_the_draft_and_reopening_reads_disk(cx: &mut TestAppContext) {
    let (directory, path, window, view) = open(cx);
    view.update(cx, |view, _| view.fail_saves_for_test(true));
    replace(cx, window, &view, "lost text");
    view.update(cx, |view, cx| view.leave(cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.session.is_none());
        assert!(view.pending.is_empty());
        assert!(view.inflight.is_none());
        assert!(view.notice().unwrap().contains("discarded"));
    });
    let config = ScanConfig::default_for_home(directory.path());
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| view.open(config, path.clone(), window, cx))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            view.read(cx).contents_for_test(cx).as_deref(),
            Some(ORIGINAL)
        )
    });
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[gpui_kit::test]
fn quit_waits_for_a_save_attempt_but_does_not_preserve_failures(cx: &mut TestAppContext) {
    let (_directory, path, window, view) = open(cx);
    let ready = Rc::new(Cell::new(false));
    let flag = Rc::clone(&ready);
    let _subscription = cx.update(|cx| {
        cx.subscribe(&view, move |_, event, _| {
            if matches!(event, EditorEvent::QuitReady) {
                flag.set(true);
            }
        })
    });
    view.update(cx, |view, _| view.fail_saves_for_test(true));
    replace(cx, window, &view, "lost at quit");
    view.update(cx, |view, cx| {
        view.quit(cx);
        assert!(view.session.is_none());
        assert!(view.inflight.is_some());
    });
    assert!(!ready.get());
    cx.run_until_parked();
    assert!(ready.get());
    assert_eq!(fs::read_to_string(path).unwrap(), ORIGINAL);
    cx.update(|cx| assert!(view.read(cx).pending.is_empty()));
}

#[gpui_kit::test]
fn another_document_opens_without_waiting_for_an_unrelated_save(cx: &mut TestAppContext) {
    let (directory, path, window, view) = open(cx);
    let other = directory.path().join("other.md");
    fs::write(&other, "other").unwrap();
    let other = fs::canonicalize(other).unwrap();
    replace(cx, window, &view, "saved before switching");
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open(
                    ScanConfig::default_for_home(directory.path()),
                    other.clone(),
                    window,
                    cx,
                );
                assert!(view.session.is_none());
                assert!(view.opening.is_some());
                assert!(view.loading);
                assert!(view.inflight.is_some());
            })
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| view.resume_open(window, cx))
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), "saved before switching");
    cx.update(|cx| {
        assert_eq!(
            view.read(cx).contents_for_test(cx).as_deref(),
            Some("other")
        )
    });
}

#[gpui_kit::test]
fn reopening_the_same_file_waits_for_its_submitted_write(cx: &mut TestAppContext) {
    let (directory, path, window, view) = open(cx);
    replace(cx, window, &view, "latest submitted text");
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.leave(cx);
                view.open(
                    ScanConfig::default_for_home(directory.path()),
                    path.clone(),
                    window,
                    cx,
                );
                assert!(!view.loading);
                assert!(view.opening.is_some());
                assert!(view.session.is_none());
            })
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| view.resume_open(window, cx))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            view.read(cx).contents_for_test(cx).as_deref(),
            Some("latest submitted text")
        )
    });
}

#[gpui_kit::test]
fn native_shutdown_saves_latest_text_without_gui_callbacks_or_stale_overwrites(
    cx: &mut TestAppContext,
) {
    let (_directory, path, window, view) = open(cx);
    replace(cx, window, &view, "older");
    view.update(cx, |view, cx| view.request_save(cx));
    let (older, gate) = cx.update(|cx| {
        let view = view.read(cx);
        (
            Arc::clone(&view.inflight.as_ref().unwrap().snapshot),
            Arc::clone(&view.save_gate),
        )
    });
    replace(cx, window, &view, "latest at native quit");
    let _shutdown = view.update(cx, |view, cx| view.prepare_shutdown(cx));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&path).unwrap(), "latest at native quit");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    EditorWorkspace::write_snapshot(&older, &gate, editing::save).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "latest at native quit");
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.session.is_none());
        assert!(view.inflight.is_none());
        assert!(view.pending.is_empty());
    });
}

#[gpui_kit::test]
fn queued_submissions_for_different_documents_do_not_replace_each_other(cx: &mut TestAppContext) {
    queued_documents(cx, false);
}

#[gpui_kit::test]
fn native_shutdown_attempts_all_submitted_documents(cx: &mut TestAppContext) {
    queued_documents(cx, true);
}

fn queued_documents(cx: &mut TestAppContext, native_shutdown: bool) {
    let (directory, path, window, view) = open(cx);
    let other = fs::canonicalize(directory.path()).unwrap().join("other.md");
    fs::write(&other, "other").unwrap();
    let config = ScanConfig::default_for_home(directory.path());
    let source = editing::load(&config, &other).unwrap();
    replace(cx, window, &view, "first");
    view.update(cx, |view, cx| view.request_save(cx));
    replace(cx, window, &view, "latest first document");
    view.update(cx, |view, cx| {
        view.leave(cx);
        view.enqueue_snapshot(Arc::new(Snapshot {
            epoch: 100,
            revision: 1,
            path: other.clone(),
            config,
            expected: source.version,
            text: "second document".into(),
        }));
        assert_eq!(view.pending.len(), 2);
    });
    let _shutdown = native_shutdown.then(|| view.update(cx, |view, cx| view.prepare_shutdown(cx)));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), "latest first document");
    assert_eq!(fs::read_to_string(other).unwrap(), "second document");
}

#[gpui_kit::test]
fn native_shutdown_uses_the_last_completed_baseline(cx: &mut TestAppContext) {
    let (_directory, path, window, view) = open(cx);
    replace(cx, window, &view, "first saved version");
    view.update(cx, |view, cx| view.request_save(cx));
    cx.run_until_parked();
    replace(cx, window, &view, "latest at quit");
    let _shutdown = view.update(cx, |view, cx| view.prepare_shutdown(cx));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), "latest at quit");
}

#[gpui_kit::test]
fn native_shutdown_failure_leaves_no_draft_or_recovery_file(cx: &mut TestAppContext) {
    let (directory, path, window, view) = open(cx);
    view.update(cx, |view, _| view.fail_saves_for_test(true));
    replace(cx, window, &view, "lost at shutdown");
    let _shutdown = view.update(cx, |view, cx| view.prepare_shutdown(cx));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), ORIGINAL);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    cx.update(|cx| assert!(view.read(cx).contents_for_test(cx).is_none()));
}

#[gpui_kit::test]
fn unrelated_success_does_not_erase_a_detached_failure_message(cx: &mut TestAppContext) {
    let (directory, path, window, view) = open(cx);
    let other = fs::canonicalize(directory.path()).unwrap().join("other.md");
    fs::write(&other, "other").unwrap();
    let config = ScanConfig::default_for_home(directory.path());
    let source = editing::load(&config, &other).unwrap();
    view.update(cx, |view, _| view.fail_saves_for_test(true));
    replace(cx, window, &view, "lost first draft");
    view.update(cx, |view, cx| {
        view.leave(cx);
        view.fail_saves_for_test(false);
        view.enqueue_snapshot(Arc::new(Snapshot {
            epoch: 100,
            revision: 1,
            path: other.clone(),
            config,
            expected: source.version,
            text: "saved other note".into(),
        }));
    });
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), ORIGINAL);
    assert_eq!(fs::read_to_string(other).unwrap(), "saved other note");
    cx.update(|cx| assert!(view.read(cx).notice().unwrap().contains("discarded")));
}

fn assert_visible_status(cx: &mut TestAppContext, window: AnyWindowHandle, expected: &str) {
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("editor-save-status").label(), Some(expected));
        })
        .unwrap();
}

#[gpui_kit::test]
fn visible_status_tracks_saving_failure_and_latest_buffer_retry(cx: &mut TestAppContext) {
    let (_directory, path, window, view) = open(cx);
    assert_visible_status(cx, window, "Saved");
    replace(cx, window, &view, "first change");
    assert_visible_status(cx, window, "Unsaved");
    view.update(cx, |view, cx| view.request_save(cx));
    assert_visible_status(cx, window, "Saving...");
    cx.run_until_parked();
    assert_visible_status(cx, window, "Saved");
    view.update(cx, |view, _| view.fail_saves_for_test(true));
    replace(cx, window, &view, "failed change");
    view.update(cx, |view, cx| view.request_save(cx));
    cx.run_until_parked();
    assert_visible_status(cx, window, "Save failed");
    replace(cx, window, &view, "latest retry contents");
    assert_visible_status(cx, window, "Unsaved");
    view.update(cx, |view, cx| view.request_save(cx));
    cx.run_until_parked();
    assert_visible_status(cx, window, "Save failed");
    view.update(cx, |view, _| view.fail_saves_for_test(false));
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("retry-editor-save", cx);
            assert!(view.read(cx).is_focused(window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_visible_status(cx, window, "Saved");
    assert_eq!(fs::read_to_string(path).unwrap(), "latest retry contents");
}

#[gpui_kit::test]
fn long_save_errors_wrap_without_hiding_source_header_or_retry(cx: &mut TestAppContext) {
    let (_directory, _, window, view) = open(cx);
    view.update(cx, |view, cx| {
        view.session.as_mut().unwrap().error =
            Some("A detailed filesystem permission failure. ".repeat(100));
        cx.notify();
    });
    cx.run_until_parked();
    let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
    let pane = visual.debug_bounds("document-editor").unwrap();
    let filename = visual.debug_bounds("document-name").unwrap();
    let error = visual.debug_bounds("editor-save-error").unwrap();
    assert!(filename.right() <= pane.right());
    assert!(error.size.height <= gpui_kit::px(128.0));
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let retry = window.find("retry-editor-save");
            assert!(retry.visible());
            assert!(retry.bounds().top() >= error.top());
            assert!(retry.bounds().bottom() <= error.bottom());
            let input_id = view
                .read(cx)
                .session
                .as_ref()
                .unwrap()
                .text
                .entity_id()
                .as_u64();
            assert!(window.find(("input", input_id)).bounds().size.height > gpui_kit::px(100.0));
            assert_eq!(view.read(cx).started_saves, 0);
        })
        .unwrap();
}

#[gpui_kit::test]
fn external_changes_fail_without_overwriting_and_departure_loses_the_draft(
    cx: &mut TestAppContext,
) {
    let (_directory, path, window, view) = open(cx);
    replace(cx, window, &view, "unsaved draft");
    fs::write(&path, "external version").unwrap();
    view.update(cx, |view, cx| view.leave(cx));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(path).unwrap(), "external version");
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.notice().unwrap().contains("Conflict"));
        assert!(view.session.is_none());
    });
}
