use std::fs;

use gpui_kit::{
    Focusable as _, Modifiers, TestAppContext, VisualTestContext, test::TestWindowExt as _,
};

use super::{
    tests::{open_indexed_test_launcher, settle_preview},
    *,
};

fn row_action(
    cx: &mut TestAppContext,
    launcher: &Entity<Launcher>,
    index: usize,
    action: &str,
) -> SharedString {
    cx.update(|cx| result_action_id(action, &launcher.read(cx).results[index].canonical_path))
}

fn click(cx: &mut TestAppContext, window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>) {
    let id = id.into();
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
}

fn hover_row(cx: &TestAppContext, window: AnyWindowHandle, index: usize) {
    let mut visual = VisualTestContext::from_window(window, cx);
    let selector = match index {
        0 => "result-content-0",
        1 => "result-content-1",
        _ => unreachable!("two-row fixture"),
    };
    let bounds = visual.debug_bounds(selector).expect("result content");
    visual.simulate_mouse_move(bounds.center(), None, Modifiers::none());
}

#[gpui_kit::test]
fn new_button_sits_beside_search_and_uses_the_existing_confirmation_dialog(
    cx: &mut TestAppContext,
) {
    let (_directory, _, window, launcher) = open_indexed_test_launcher(cx, &[("alpha.md", "body")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "alpha");
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    settle_preview(cx);
    let mut visual = VisualTestContext::from_window(window, cx);
    let search = visual.debug_bounds("launcher-query").expect("search input");
    let toolbar = visual.debug_bounds("launcher-search-bar").expect("toolbar");
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let button = window.find("workspace-new-note");
            assert_eq!(button.label(), Some("New Markdown note"));
            assert!(button.bounds().left() >= search.right());
            assert!(button.bounds().right() <= toolbar.right());
            assert!(button.bounds().top() >= toolbar.top());
        })
        .unwrap();
    click(cx, window, "workspace-new-note");
    cx.update(|cx| {
        let launcher = launcher.read(cx);
        let dialog = launcher.creation.as_ref().unwrap().read(cx);
        assert_eq!(dialog.filename_for_test(cx), "alpha.md");
        assert_eq!(launcher.selected_index, Some(0));
    });
    cx.simulate_keystrokes(window, "escape");
    window
        .update(cx, |_, window, cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.creation.is_none());
            assert_eq!(launcher.query.read(cx).value().as_str(), "alpha");
            assert_eq!(launcher.selected_index, Some(0));
            assert!(launcher.query.read(cx).focus_handle(cx).is_focused(window));
        })
        .unwrap();
}

#[gpui_kit::test]
fn hover_reveals_only_row_actions_without_selection_or_layout_jumps(cx: &mut TestAppContext) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
    cx.run_until_parked();
    let edit_alpha = row_action(cx, &launcher, 0, "edit");
    let edit_beta = row_action(cx, &launcher, 1, "edit");
    let copy_beta = row_action(cx, &launcher, 1, "copy");
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                !window
                    .try_find(edit_alpha.clone())
                    .is_some_and(|button| button.visible())
            );
            assert!(
                !window
                    .try_find(edit_beta.clone())
                    .is_some_and(|button| button.visible())
            );
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(window, cx);
    let before = visual.debug_bounds("result-content-1").unwrap();
    hover_row(cx, window, 1);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find(edit_beta).visible());
            assert!(
                !window
                    .try_find(edit_alpha)
                    .is_some_and(|button| button.visible())
            );
            assert!(
                window
                    .find(copy_beta.clone())
                    .label()
                    .unwrap()
                    .starts_with("Copy unavailable:")
            );
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(window, cx);
    assert_eq!(visual.debug_bounds("result-content-1").unwrap(), before);
    click(cx, window, copy_beta);
    cx.update(|cx| {
        let launcher = launcher.read(cx);
        assert!(launcher.selected_index.is_none());
        assert_eq!(launcher.preview, PreviewState::Empty);
        assert!(launcher.editor.read(cx).path().is_none());
    });
}

#[gpui_kit::test]
fn disabled_copy_on_another_row_does_not_select_or_reload_anything(cx: &mut TestAppContext) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    settle_preview(cx);
    let copy_alpha = row_action(cx, &launcher, 0, "copy");
    let copy_beta = row_action(cx, &launcher, 1, "copy");
    let generation = cx.update(|cx| launcher.read(cx).preview_generation.load(Ordering::Relaxed));
    hover_row(cx, window, 1);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find(copy_alpha).label(), Some("Copy Markdown body"));
            assert!(
                window
                    .find(copy_beta.clone())
                    .label()
                    .unwrap()
                    .starts_with("Copy unavailable:")
            );
        })
        .unwrap();
    click(cx, window, copy_beta);
    window
        .update(cx, |_, window, cx| {
            let launcher = launcher.read(cx);
            assert_eq!(launcher.selected_index, Some(0));
            assert_eq!(
                launcher.preview_generation.load(Ordering::Relaxed),
                generation
            );
            assert!(launcher.query.read(cx).focus_handle(cx).is_focused(window));
        })
        .unwrap();
}

#[gpui_kit::test]
fn edit_button_targets_its_row_and_keeps_the_editor_focused(cx: &mut TestAppContext) {
    let (directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    settle_preview(cx);
    let edit_beta = row_action(cx, &launcher, 1, "edit");
    hover_row(cx, window, 1);
    click(cx, window, edit_beta.clone());
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let launcher = launcher.read(cx);
            assert_eq!(launcher.selected_index, Some(1));
            assert!(
                launcher
                    .editor
                    .read(cx)
                    .path()
                    .unwrap()
                    .ends_with("beta.md")
            );
            assert!(launcher.editor.read(cx).is_focused(window, cx));
            assert_eq!(window.find(edit_beta).label(), Some("Editing note"));
            assert_eq!(window.find("editor-save-status").label(), Some("Saved"));
        })
        .unwrap();
    cx.simulate_keystrokes(window, "z escape");
    cx.run_until_parked();
    assert_eq!(
        fs::read_to_string(directory.path().join("notes/beta.md")).unwrap(),
        "betaz"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("notes/alpha.md")).unwrap(),
        "alpha"
    );
}

#[gpui_kit::test]
fn copy_button_uses_current_disk_content_without_bubbling_to_row_selection(
    cx: &mut TestAppContext,
) {
    let (directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "indexed beta")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "up");
    settle_preview(cx);
    let copy = row_action(cx, &launcher, 1, "copy");
    let generation = cx.update(|cx| launcher.read(cx).preview_generation.load(Ordering::Relaxed));
    fs::write(directory.path().join("notes/beta.md"), "latest beta").unwrap();
    click(cx, window, copy);
    cx.update(|cx| {
        let launcher = launcher.read(cx);
        assert!(launcher.copying);
        assert_eq!(launcher.selected_index, Some(1));
        assert_eq!(
            launcher.preview_generation.load(Ordering::Relaxed),
            generation
        );
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("latest beta")
        );
        assert!(launcher.read(cx).selected_index.is_none());
        assert!(!launcher.read(cx).copying);
    });
}

#[gpui_kit::test]
fn copy_controls_stay_disabled_during_loading_and_editing(cx: &mut TestAppContext) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    let copy = row_action(cx, &launcher, 0, "copy");
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find(copy.clone()).label(),
                Some("Copy unavailable: Wait for a ready preview before copying")
            );
        })
        .unwrap();
    click(cx, window, copy.clone());
    cx.update(|cx| {
        assert!(matches!(
            launcher.read(cx).preview,
            PreviewState::Loading(_)
        ))
    });
    settle_preview(cx);
    cx.simulate_keystrokes(window, "cmd-e");
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find(copy.clone()).label(),
                Some("Copy unavailable: Finish editing before copying Markdown")
            );
        })
        .unwrap();
    click(cx, window, copy);
    window
        .update(cx, |_, window, cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.editor.read(cx).is_focused(window, cx));
            assert_eq!(
                launcher.editor.read(cx).contents_for_test(cx).as_deref(),
                Some("alpha")
            );
            assert!(!launcher.copying);
        })
        .unwrap();
}

#[gpui_kit::test]
fn failed_copy_keeps_search_focus_and_reports_the_reason(cx: &mut TestAppContext) {
    let (directory, _, window, launcher) = open_indexed_test_launcher(cx, &[("alpha.md", "alpha")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    settle_preview(cx);
    let copy = row_action(cx, &launcher, 0, "copy");
    fs::remove_file(directory.path().join("notes/alpha.md")).unwrap();
    click(cx, window, copy);
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let launcher = launcher.read(cx);
            assert!(!launcher.copying);
            assert_eq!(launcher.selected_index, Some(0));
            assert!(launcher.query.read(cx).focus_handle(cx).is_focused(window));
            assert!(
                window
                    .find("workspace-status")
                    .label()
                    .unwrap()
                    .starts_with("Copy failed:")
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn obsolete_pending_and_modal_row_callbacks_cannot_act_on_the_workspace(cx: &mut TestAppContext) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    settle_preview(cx);
    let (path, generation) = cx.update(|cx| {
        let launcher = launcher.read(cx);
        (
            launcher.results[0].canonical_path.clone(),
            launcher.search_generation,
        )
    });
    window
        .update(cx, |_, window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.start_search("alpha".into(), cx);
                launcher.edit_clicked_result(&path, generation, window, cx);
                launcher.copy_clicked_result(&path, generation, window, cx);
                assert!(launcher.editor.read(cx).path().is_none());
                assert!(!launcher.copying);
            })
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.edit_clicked_result(&path, generation, window, cx);
                launcher.copy_clicked_result(&path, generation, window, cx);
                assert!(launcher.editor.read(cx).path().is_none());
                launcher.new_note(&NewNote, window, cx);
                launcher.edit_clicked_result(&path, launcher.search_generation, window, cx);
                launcher.copy_clicked_result(&path, launcher.search_generation, window, cx);
                assert!(launcher.creation.is_some());
                assert!(!launcher.copying);
                assert!(launcher.editor.read(cx).path().is_none());
            })
        })
        .unwrap();
}

#[gpui_kit::test]
fn new_button_cancel_restores_editor_or_search_without_discarding_the_draft(
    cx: &mut TestAppContext,
) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down cmd-e");
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "z");
    click(cx, window, "workspace-new-note");
    cx.simulate_keystrokes(window, "escape");
    window
        .update(cx, |_, window, cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.editor.read(cx).is_focused(window, cx));
            assert_eq!(
                launcher.editor.read(cx).contents_for_test(cx).as_deref(),
                Some("alphaz")
            );
            let query = launcher.query.clone();
            query.update(cx, |input, cx| input.focus(window, cx));
        })
        .unwrap();
    click(cx, window, "workspace-new-note");
    cx.simulate_keystrokes(window, "escape");
    window
        .update(cx, |_, window, cx| {
            let launcher = launcher.read(cx);
            assert!(launcher.query.read(cx).focus_handle(cx).is_focused(window));
            assert!(!launcher.editor.read(cx).is_focused(window, cx));
            assert_eq!(
                launcher.editor.read(cx).contents_for_test(cx).as_deref(),
                Some("alphaz")
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn unavailable_scan_settings_disable_creation_and_edit_controls(cx: &mut TestAppContext) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha")]);
    cx.run_until_parked();
    launcher.update(cx, |launcher, cx| {
        launcher.scan_config = None;
        cx.notify();
    });
    hover_row(cx, window, 0);
    let edit = row_action(cx, &launcher, 0, "edit");
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("workspace-new-note").label(),
                Some("New note unavailable without active scan settings")
            );
            assert!(
                window
                    .find(edit.clone())
                    .label()
                    .unwrap()
                    .starts_with("Edit unavailable:")
            );
        })
        .unwrap();
    click(cx, window, edit);
    click(cx, window, "workspace-new-note");
    cx.update(|cx| {
        let launcher = launcher.read(cx);
        assert!(launcher.creation.is_none());
        assert!(launcher.selected_index.is_none());
        assert!(launcher.editor.read(cx).path().is_none());
    });
}

#[gpui_kit::test]
fn copy_requests_are_deduplicated_and_old_completions_cannot_clear_new_work(
    cx: &mut TestAppContext,
) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    settle_preview(cx);
    window
        .update(cx, |_, window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.activate_selected(&ActivateSelected, window, cx);
                let generation = launcher.copy_generation;
                launcher.activate_selected(&ActivateSelected, window, cx);
                assert_eq!(launcher.copy_generation, generation);
                assert!(launcher.copying);
                launcher.complete_copy(
                    generation.wrapping_sub(1),
                    Ok("obsolete".into()),
                    window,
                    cx,
                );
                assert!(launcher.copying);
                launcher.cancel_copy();
                assert!(!launcher.copying);
            })
        })
        .unwrap();
}

#[gpui_kit::test]
fn new_note_cancels_a_pending_editor_without_stealing_confirmation_focus(cx: &mut TestAppContext) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    window
        .update(cx, |_, window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.edit_selected(&EditSelected, window, cx)
            });
            assert!(!launcher.read(cx).editor.read(cx).has_draft());
            window.render_frame(cx);
            window.click("workspace-new-note", cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "beta.md");
    cx.update(|cx| {
        let launcher = launcher.read(cx);
        assert_eq!(
            launcher
                .creation
                .as_ref()
                .unwrap()
                .read(cx)
                .filename_for_test(cx),
            "beta.md"
        );
        assert!(launcher.editor.read(cx).path().is_none());
        assert!(launcher.editor.read(cx).contents_for_test(cx).is_none());
    });
    cx.simulate_keystrokes(window, "escape");
    window
        .update(cx, |_, window, cx| {
            assert!(
                launcher
                    .read(cx)
                    .query
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn tab_reaches_new_and_selected_actions_without_focusing_hidden_row_buttons(
    cx: &mut TestAppContext,
) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    settle_preview(cx);
    let edit = row_action(cx, &launcher, 0, "edit");
    window
        .update(cx, |_, window, cx| {
            window.press("tab", cx);
            window.render_frame(cx);
            assert_eq!(window.find("workspace-new-note").focused(), Some(true));
            window.press("tab", cx);
            window.render_frame(cx);
            assert_eq!(window.find(edit).focused(), Some(true));
            window.press("space", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            assert!(launcher.read(cx).editor.read(cx).is_focused(window, cx));
        })
        .unwrap();
}

#[gpui_kit::test]
fn arrows_from_a_row_button_return_focus_to_search_before_that_button_hides(
    cx: &mut TestAppContext,
) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    settle_preview(cx);
    window
        .update(cx, |_, window, cx| {
            window.press("tab", cx);
            window.press("tab", cx);
            window.press("down", cx);
            assert_eq!(launcher.read(cx).selected_index, Some(1));
            assert!(
                launcher
                    .read(cx)
                    .query
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
        })
        .unwrap();
    cx.simulate_keystrokes(window, "beta");
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(launcher.read(cx).query.read(cx).value().as_str(), "beta"));
}

#[gpui_kit::test]
fn boundary_shortcuts_traverse_the_accessible_result_list(cx: &mut TestAppContext) {
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "ctrl-end");
    settle_preview(cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let launcher = launcher.read(cx);
            assert_eq!(launcher.selected_index, Some(1));
            assert!(launcher.query.read(cx).focus_handle(cx).is_focused(window));
        })
        .unwrap();
    cx.simulate_keystrokes(window, "ctrl-home");
    settle_preview(cx);
    cx.update(|cx| assert_eq!(launcher.read(cx).selected_index, Some(0)));
}

#[gpui_kit::test]
fn a_removed_result_does_not_leave_keyboard_focus_on_an_invisible_action(cx: &mut TestAppContext) {
    let (_directory, database_path, window, launcher) =
        open_indexed_test_launcher(cx, &[("alpha.md", "alpha"), ("beta.md", "beta")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    settle_preview(cx);
    window
        .update(cx, |_, window, cx| {
            window.press("tab", cx);
            window.press("tab", cx);
        })
        .unwrap();
    IndexDatabase::open(&database_path)
        .unwrap()
        .clear_documents()
        .unwrap();
    launcher.update(cx, |launcher, cx| launcher.start_search("".into(), cx));
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "beta");
    cx.update(|cx| assert_eq!(launcher.read(cx).query.read(cx).value().as_str(), "beta"));
}

#[gpui_kit::test]
fn long_filenames_and_diagnostics_stay_inside_their_panes(cx: &mut TestAppContext) {
    let filename = format!("{}.md", "long-note-name-".repeat(14));
    let (_directory, _, window, launcher) =
        open_indexed_test_launcher(cx, &[(filename.as_str(), "body")]);
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "down");
    settle_preview(cx);
    launcher.update(cx, |launcher, cx| {
        launcher.action_status = Some(format!(
            "Copy failed: {}",
            "Detailed permission failure. ".repeat(100)
        ));
        cx.notify();
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window, cx);
    let pane = visual.debug_bounds("document-preview").unwrap();
    let heading = visual.debug_bounds("document-name").unwrap();
    let status = visual.debug_bounds("workspace-status").unwrap();
    assert!(heading.right() <= pane.right());
    assert!(heading.left() >= pane.left());
    assert!(status.size.height <= px(96.0));
    assert!(pane.size.height > px(200.0));
}
