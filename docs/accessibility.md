# Accessibility

Markdown Launcher is designed for keyboard-first use. Accessibility work is an
ongoing product requirement, not a claim of certification against a particular
standard. Native assistive-technology behavior must be verified on supported
platforms in addition to automated tests.

## Current support

- Every primary launcher workflow is available without a mouse: search, result
  selection, preview navigation, copy, note creation, editing, index rebuild,
  settings, and quit.
- Search results expose listbox and option roles with selected state. The search
  and creation controls expose a labeled toolbar role.
- Icon-only row actions and the New control have descriptive accessible labels.
  Unavailable actions include the reason in their label and tooltip.
- Indexing, configuration, and editor-save feedback use status roles so state
  changes are not represented by color alone.
- Keyboard-focus rings remain visible on the dark workspace controls. Fixed
  palettes have automated contrast checks for enabled button labels, caret, and
  text-selection colors.
- Result selection is explicit. Hover does not change selection, copy content,
  or hide the launcher.

Run the installed executable with `--shortcuts` for the keyboard reference
organized by focus context. The same reference is available during development:

```sh
cargo run -- --shortcuts
```

## Known limitations

- GPUI Kit currently does not expose the native disabled accessibility state for
  these controls. Markdown Launcher blocks activation and supplies a descriptive
  unavailable reason, but some assistive technologies may not announce the
  control as disabled.
- The rendered Markdown preview supports text selection and keyboard scrolling,
  but it is not yet a fully semantic rich-document view. Source editing remains
  available for access to the complete Markdown text.
- Automated interaction and palette tests cannot establish VoiceOver reading
  order, announcements, or visible focus behavior. These require a packaged app
  and native manual testing.
- macOS 15 or later is the primary supported desktop platform. Assistive-
  technology behavior on Linux has not received the same native verification.

These limits should not be hidden by documentation or worked around by making a
control appear available when its action cannot safely run.

## Native verification

Use disposable fixture notes with no private content. Build and open the local
application, enable VoiceOver, and check at least the following:

1. Move from search to New and the selected result's actions with Tab and
   Shift+Tab. Invisible and unavailable row actions must not receive focus.
2. Confirm search, the results list, selected options, icon actions, status
   messages, preview, editor, and save status have understandable names.
3. Select results with Up and Down, then use Ctrl+Home and Ctrl+End. Selection
   and the announced note must agree.
4. Activate focused controls with Space or Enter and verify that focus moves to
   the expected dialog, preview, editor, or search field.
5. Trigger loading, empty, unavailable-copy, invalid-note, and failed-save states.
   Information and recovery actions must remain understandable without color.
6. Inspect focus rings, caret, and text selection in the dark workspace and the
   light New Note dialog at normal and increased display scaling.
7. Repeat Escape transitions from the dialog, editor, preview, and search. No
   hidden control should retain keyboard focus.

The broader fixture setup and expected behavior are documented in
[Manual setup and native testing](../manual_setup.md#workspace-buttons-focus-and-long-content).

## Reporting an accessibility problem

Use the dedicated accessibility issue form. Include:

- The app version or commit
- Operating system and assistive technology version
- The focused control and exact keyboard sequence
- Expected name, role, state, focus movement, or announcement
- Actual behavior and whether a mouse-only workaround exists

Remove note contents, usernames, and personal paths from reports, screenshots,
and recordings. The `--diagnostics` command intentionally omits those values and
is safe to review before attaching its output.
