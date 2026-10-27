# Manual Setup and Native Testing

This guide provides two local workflows: Part 1 runs the application from
source with `cargo run`, while Part 2 builds an optimized release executable
and a local macOS `.app`. The configuration and testing sections after those
parts apply to both workflows. Developer ID signing, notarization, installers,
and public release automation are not covered.

## Prerequisites

- Rust 1.99 or later. The checked-in `rust-toolchain.toml` selects the expected
  toolchain.
- The native dependencies required by
  [GPUI Kit](https://gpui-kit.com/docs/installation).
- On macOS, macOS 15 or later and the Xcode Command Line Tools.
- SQLite does not need to be installed separately because the project builds a
  bundled SQLite library.
- On Linux, native global-shortcut support requires an X11 session; a Wayland
  session may report that the shortcut is unavailable.

Check the toolchain and project before building:

```sh
rustc --version
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

## Part 1: Development testing with `cargo run`

Use this workflow while changing code or performing quick local tests. Cargo
builds a development executable and starts it immediately:

```sh
cargo run
```

The development executable is written to:

- macOS and Linux: `target/debug/markdown-snippet-manager`
- Windows: `target\debug\markdown-snippet-manager.exe`

Keep the command running in the foreground so startup errors remain visible.
Use the app's Quit action to attempt pending edit saves. Ctrl+C or force-killing
may lose unsaved edits; there is no draft recovery. The process must remain running
for the global shortcut, filesystem watcher, and background index to remain available.

A typical development cycle is:

1. Run `cargo run`.
2. Exercise the launcher and modify test Markdown files from another terminal.
3. Quit through the application after checking save status.
4. Change the code or configuration.
5. Run `cargo run` again.

Cargo recompiles only what changed. This workflow is faster for iteration, but
it does not test the optimized release executable.

## Part 2: Testing the release build

Stop any `cargo run` process before starting a release build. Development and
release builds use the same global shortcut, configuration, and SQLite index.
Running both at once can cause shortcut and database conflicts.

### Build the local macOS application

On macOS, run the checked-in packaging script:

```sh
./scripts/build-macos-app.sh
```

The script:

1. Builds the optimized executable with `cargo build --release --locked`.
2. Creates `dist/Markdown Launcher.app`.
3. Adds `Info.plist` metadata and the application icon.
4. Configures the bundle as a background accessory application without a Dock
   icon.
5. Applies and verifies an ad-hoc code signature for local use.

The packaged application displays a Markdown Launcher item in the macOS menu
bar. It does not display a Dock icon. Launch at Login is disabled by default
and remains unavailable until the app is copied into `/Applications` or the
user's `Applications` directory.

Build and immediately open it with:

```sh
./scripts/build-macos-app.sh --open
```

Or open an existing build with:

```sh
open "dist/Markdown Launcher.app"
```

To install it, stop any running copy and drag `Markdown Launcher.app` from the
`dist` directory into `/Applications`. It can also be copied from a terminal:

```sh
ditto "dist/Markdown Launcher.app" "/Applications/Markdown Launcher.app"
```

The bundle is ad-hoc signed for local use. It is not Developer ID signed or
notarized and should not be treated as a public distribution artifact. Copying
it into `/Applications` does not make it start automatically after login; the
user must explicitly enable **Launch at Login** from the menu bar item.

A release build targets the current architecture. On an Apple Silicon Mac the
bundle contains an `arm64` executable, not a universal Intel and Apple Silicon
binary. The bundle declares macOS 15 as its minimum system version.

### Build only the raw release executable

To build without creating a macOS application bundle:

```sh
cargo build --release
```

The output is:

- macOS and Linux: `target/release/markdown-snippet-manager`
- Windows: `target\release\markdown-snippet-manager.exe`

Run it on macOS or Linux with:

```sh
./target/release/markdown-snippet-manager
```

On Windows, run:

```powershell
.\target\release\markdown-snippet-manager.exe
```

Windows installers, Linux desktop packages, universal macOS binaries, Developer
ID signing, notarization, and release automation remain separate future work.

## Configuration shared by both workflows

`cargo run`, the raw release executable, and the macOS `.app` use the same
configuration format and runtime behavior.

### Application files

The application stores `config.toml` and the disposable `index.sqlite3` in the
platform application directories.

#### macOS

```text
Configuration:
$HOME/Library/Application Support/com.markdown-launcher.MarkdownSnippetManager/config.toml

Index:
$HOME/Library/Application Support/com.markdown-launcher.MarkdownSnippetManager/index.sqlite3
```

#### Linux

```text
Configuration:
$XDG_CONFIG_HOME/markdownsnippetmanager/config.toml
or $HOME/.config/markdownsnippetmanager/config.toml

Index:
$XDG_DATA_HOME/markdownsnippetmanager/index.sqlite3
or $HOME/.local/share/markdownsnippetmanager/index.sqlite3
```

#### Windows

```text
Configuration:
%APPDATA%\markdown-launcher\MarkdownSnippetManager\config\config.toml

Index:
%APPDATA%\markdown-launcher\MarkdownSnippetManager\data\index.sqlite3
```

The application creates a default configuration if one does not exist. On
macOS, choose **Settings…** from the menu bar or press Command+, while the
launcher is visible to edit the supported values. The TOML file remains
available for manual editing. Changes saved through either method require an
application restart.

SQLite is a derived search index. Deleting it does not delete or modify any
Markdown source files.

### Default configuration

The generated configuration is equivalent to:

```toml
global_shortcut = "cmdorctrl+shift+Space"

[scan]
roots = ["<operating-system home directory>"]
extensions = ["md", "markdown"]
excluded_directory_names = [
  ".git",
  ".hg",
  ".svn",
  ".cache",
  ".Trash",
  "node_modules",
  "target",
]
excluded_globs = [
  "Library/Caches",
  "Library/Caches/**",
]
max_file_size_bytes = 2097152
follow_symbolic_links = false
```

The home directory is resolved through the operating system. It is not stored
as the literal placeholder shown above.

These defaults mean:

- `.md` and `.markdown` files are discovered recursively.
- Extension matching is case-insensitive.
- Common generated, repository-internal, trash, and cache directories are
  skipped.
- Files larger than 2 MiB are skipped and reported as indexing issues.
- Symbolic links are not followed.
- All indexing and searching remain local.

### Use a controlled scan root for testing

The default home-directory scan may be large. For predictable testing, create a
small fixture directory and configure it before launching the application.

```sh
TEST_ROOT="$HOME/markdown-launcher-smoke-test"

rm -rf "$TEST_ROOT"
mkdir -p "$TEST_ROOT/nested" "$TEST_ROOT/node_modules/package"

cat > "$TEST_ROOT/alpha.md" <<'EOF'
---
title: Alpha snippet
tags: [alpha, testing]
---

Original clipboard body
EOF

cat > "$TEST_ROOT/nested/beta.markdown" <<'EOF'
# Beta document

Unique beta content
EOF

echo "must remain excluded" \
  > "$TEST_ROOT/node_modules/package/excluded.md"
```

Set `roots` in `config.toml` to the absolute path represented by `$TEST_ROOT`:

```toml
[scan]
roots = ["/absolute/path/to/markdown-launcher-smoke-test"]
```

Do not use `~` in the TOML path. Shell home-directory expansion does not occur
inside configuration files.

### Customize the configuration

The native settings window supports scan roots through a folder picker, the
global shortcut, extensions, directory and glob exclusions, the file-size
limit, and symbolic-link traversal. It shows the recommended defaults and can
restore them to the draft without saving automatically. It validates the
complete configuration and atomically replaces `config.toml` only when the
values are valid. Saving rewrites
the file in normalized TOML format, so manually added comments are not
preserved.

A complete custom example is:

```toml
global_shortcut = "alt+shift+Space"

[scan]
roots = [
  "/absolute/path/to/notes",
  "/absolute/path/to/project-documentation",
]
extensions = ["md", "markdown", "mdown"]
excluded_directory_names = [
  ".git",
  ".cache",
  "node_modules",
  "target",
  "vendor",
]
excluded_globs = [
  "Library/Caches",
  "Library/Caches/**",
  "archive/private/**",
]
max_file_size_bytes = 4194304
follow_symbolic_links = false
```

#### Global shortcut

The default is Command+Shift+Space on macOS and Ctrl+Shift+Space on other
platforms:

```toml
global_shortcut = "cmdorctrl+shift+Space"
```

Other examples include:

```toml
global_shortcut = "alt+Space"
global_shortcut = "cmdorctrl+shift+KeyM"
```

If another application owns the shortcut, the launcher displays a startup
warning and remains usable through its initially opened window. Choose another
shortcut and restart the application.

#### Scan roots

`roots` accepts one or more directories. Use absolute paths for predictable
behavior. Duplicate roots are removed during configuration normalization.

The scanner remains within each configured root after path canonicalization.
Symbolic links are disabled by default to avoid cycles and unexpected traversal
outside those roots.

#### Extensions

Extensions may be written with or without a leading dot. They are trimmed,
lowercased, and deduplicated. At least one extension is required.

#### Directory exclusions

`excluded_directory_names` matches individual directory names at any depth.
For example, `node_modules` excludes every directory with that name below a
scan root.

#### Glob exclusions

`excluded_globs` matches paths relative to each configured root. Use `/` as the
separator in patterns. Add both a directory and its descendants when the
intent should be explicit:

```toml
excluded_globs = ["private", "private/**"]
```

#### File-size limit

`max_file_size_bytes` controls the largest file that can be indexed or copied.
It must be greater than zero. Oversized files are skipped without stopping the
rest of the scan.

#### Symbolic links

Set the following only when traversal through symbolic links is intentional:

```toml
follow_symbolic_links = true
```

Canonical paths are still checked against the configured root boundary.

#### Invalid configuration

An invalid global shortcut uses the default shortcut and displays a startup
warning while preserving all valid scan settings.

If the configuration cannot be parsed or its scan settings are invalid, startup
stops and reports the error on stderr. Fix the file and restart the application.
The invalid file is never overwritten, and errors never cause indexing to fall
back to the home directory.

### Supported Markdown metadata

Files may contain YAML frontmatter:

```markdown
---
id: optional-stable-id
title: Display title
tags: [rust, async]
---

Markdown body copied to the clipboard
```

- `title` is optional and falls back to the filename without its extension.
- `tags` may be one string or a list.
- `id` is optional.
- Files without frontmatter remain valid.
- Frontmatter is indexed as metadata but is not copied to the clipboard.

Malformed frontmatter is isolated to that file and shown as an indexing issue.
If a previously valid file becomes malformed, its stale search entry is
removed.

## Features to expect while testing

### Startup and indexing

- The launcher opens without waiting for the recursive scan to finish.
- Existing SQLite results are available while startup reconciliation runs.
- The footer reports files checked, files updated, and issue counts.
- Search remains responsive while indexing continues.

### Search

- Typing searches titles, tags, and Markdown bodies through SQLite FTS5.
- Title matches rank above tag matches, which rank above body-only matches.
- Result counts are bounded.
- A slower old asynchronous query cannot replace newer search results.

### Keyboard workflow

- Search results start unselected. Changing the query clears selection and preview.
- With no selection, Down selects the first result and Up selects the last.
  Further navigation stays within the result range.
- Clicking selects a result and previews it without copying or hiding. Hover
  does not change selection.
- Enter focuses the selected, loaded read-only preview; it no longer copies.
- In preview, Up/Down and Page Up/Down scroll; Home/End or Command+Up/Down jump
  to the beginning/end without changing the selected result.
- Command/Ctrl+Shift+C reads the selected file again from disk, copies its current
  Markdown body, resets the query, and hides. It is disabled with no selection,
  during loading, after a preview error, or while an editor remains open.
- Command/Ctrl+E explicitly edits the selected file's complete source, including
  frontmatter. Editor arrows move the cursor. Command/Ctrl+S saves immediately;
  autosave starts after 500 ms of inactivity. Enter in search refocuses an open editor.
- Escape from editing submits changes, shows read-only preview, and focuses
  search with the query/result preserved. Escape from preview also focuses search.
  Escape from search resets the query and hides without changing the clipboard.
- Command/Ctrl+A and Command/Ctrl+C retain normal input behavior in search.
  In preview they select the entire rendered document and copy selected plain
  text without hiding, including offscreen content. Drag selects text,
  double-click selects a word, and Shift-click extends selection.
- Command/Ctrl+N opens New Note with an editable full filename and destination.
  Enter confirms, Escape cancels, and errors stay in the dialog. After success,
  the search clears and the new note opens in the editor after its initial heading.
- Command/Ctrl+Shift+R requests a complete index rebuild.
- Command+Q quits the resident application on macOS.

### Workspace controls

- **New** beside search opens the same confirmation dialog as Command/Ctrl+N.
- Hover or keyboard selection reveals compact Edit/Copy icon buttons on a result.
  Space is reserved so result text does not move on hover. Hover never selects.
- Edit selects its own row and opens source editing. Copy works only on the already
  selected row with a ready preview; it stays disabled during editing or copying.
- Tooltips explain shortcuts and unavailable states. Only selected enabled row
  actions participate in Tab navigation; action clicks never also click the row.
- The document pane shows mode and filename; hovering its filename shows the full
  path. Save status stays above source, and long errors scroll with Retry Save
  kept outside the scrolling details. The footer also wraps within bounded height.

### New-note creation

- Every creation opens a confirmation dialog; no separate managed-folder setup
  or restart is required.
- The current query supplies a suggested filename, or `Untitled.md` with an
  empty query (using a configured extension if `.md` is disabled).
- The destination defaults to the last successful eligible location. On first
  use, the OS Documents subfolder `Markdown Launcher Notes` is suggested only
  when inside an active scan root and not excluded; otherwise use an eligible root.
- **Choose...** opens a native macOS folder sheet. Any eligible folder under an
  active root may be selected. Errors for outside-root/excluded/symlink paths
  appear in the dialog; those paths are never written.
- Missing subfolders are created only after confirmation. Invalid names,
  unsupported extensions, duplicate names, permissions, and filesystem errors
  leave the draft open for correction. Existing names never receive silent
  suffixes and are never overwritten.
- A note initially contains `# <filename without extension>` and a newline.
- Publication is atomic and exclusive. Cancellation and quit are disabled
  during the short creation write so it can finish before the view is removed.
- The last successful destination is persisted in `note-creation.json` next to
  `config.toml`, independently of scan settings. Existing eligible notes can be
  edited explicitly with Command/Ctrl+E.
- The created note stays included in the empty-query result set until a query
  change or deletion, even while indexing catches up or the result limit is full.
- Cancelling restores the previous query, result, and search/preview/editor focus.

### Read-only preview

- The left pane shows results; the wider right pane shows the selected document.
- No document loads until an explicit selection.
- Loading and error states appear in the preview without blocking search.
- File reading and parsing run in the background after a 100 ms debounce.
- The configured file-size limit applies to preview as well as indexing/copy.
- The complete body is read and parsed within the configured file-size limit.
  The preview is not truncated; scroll to reach the rest of the document.
- A variable-height virtual list renders only visible rows plus a small buffer.
  Huge paragraphs and code fences are split into display rows without dropping
  text. This reduces layout work, not full-document reading or memory use.
- Headings, paragraphs, lists, and code blocks have basic formatting. Rich inline
  styles and code syntax highlighting are not yet supported.
- Text selection uses logical positions in the full rendered document. Moving
  between search and preview preserves it; replacing/reloading a preview clears it.
- Frontmatter is omitted. HTML is literal text; links and images do not load resources.
- Index refreshes reload a selected file, or clear selection if it disappears.
- Normal copy copies rendered plain text; Command/Ctrl+Shift+C copies the full
  current Markdown body, retaining Markdown syntax and omitting frontmatter.

### Editing and best-effort saves

- Any eligible viewable Markdown file may be edited; app ownership is not required.
- The complete UTF-8 source is retained separately from the rendered preview.
  Existing frontmatter, BOM, and newlines are not reconstructed from display rows.
- One active draft lives in memory. Changed content is saved asynchronously with
  serialized, coalesced writes and atomic replacement. Unchanged preview browsing
  never writes files. The editor shows Unsaved, Saving, Saved, or a failure reason.
- Retry Save uses the latest active buffer. Conflicts do not allow force-overwrite;
  preserve anything needed with ordinary source copy, leave, then reopen from disk.
- Leaving editing with Escape, switching documents, changing the query, or hiding
  submits changes and drops the draft. Failed detached saves discard their bytes and leave only a reason; they
  cannot be retried or recovered. Other files remain editable while saves are pending.
- Normal quit waits for save attempts and exits even on failure. Native system quit
  is best effort within the framework's shutdown timeout. Crashes may lose edits.
  There are no checkpoint files or crash recovery; atomic-save temporary files
  are not recovery drafts.
- New Note cancellation restores an existing editor and its buffer. Opening
  Settings ends editing with the same best-effort save used when hiding.
- Ordinary editor copy/select-all uses the source input without hiding. Whole-note
  copy is disabled during editing until draft-aware copying is implemented.

### Resident popup behavior

- The launcher is borderless, centered, and non-resizable.
- Losing focus hides it, except while the New Note dialog or its native folder
  sheet is active.
- Closing it hides the window instead of terminating the resident process.
- The configured global shortcut reopens it and focuses the input.
- On macOS it runs with accessory behavior rather than as a normal Dock
  application.
- The macOS menu bar item provides **Open Launcher**, **Settings…**,
  **Rebuild Index**, **Launch at Login**, and **Quit Markdown Launcher**.
- Opening Settings hides the search popup while preserving its query and
  selection. Closing Settings leaves search hidden until the global shortcut or
  **Open Launcher** is used.
- The settings window shows the recommended defaults, can restore them to the
  unsaved draft, saves validated configuration, and clearly reports that a
  restart is required before changes take effect.
- Launch at Login remains disabled until the user enables it. When enabled,
  subsequent logins initialize the resident application without showing the
  search window.
- A second macOS process exits instead of starting another watcher, indexer, or
  global-shortcut registration.

### Filesystem synchronization

After a short debounce delay:

- Creating a supported Markdown file adds it to search.
- Modifying a file replaces its searchable content.
- Renaming a file removes the old path and indexes the new path.
- Deleting a file removes its result.
- Directory events reconcile only the affected subtree.
- Watcher failures trigger automatic retries and a reconciliation after
  reconnection.

### Diagnostics and recovery

- Parse, permission, oversized-file, and watcher failures do not stop unrelated
  files from being indexed.
- The footer shows a bounded issue summary and the first recent file-level
  diagnostic.
- A corrupt or structurally invalid SQLite index is moved aside with a
  `.corrupt-*` suffix and rebuilt from Markdown sources.
- A database created by a newer application version is not replaced.

## Native testing checklist

### Search and navigation

1. Start the application with `cargo run`, the release executable, or the
   macOS `.app`.
2. Search for `alpha`, `testing`, and `beta`.
3. Confirm title, tag, and body searches return the expected documents.
4. Clear the query and verify Up and Down move the selected row.
5. Search for `excluded` and confirm the `node_modules` fixture is absent.

### macOS menu bar and process lifecycle

1. Build the application with `./scripts/build-macos-app.sh`, copy it into
   `/Applications`, and open the installed copy.
2. Confirm the Markdown Launcher icon appears in the macOS menu bar and no Dock
   icon appears.
3. Confirm **Launch at Login** is initially unchecked and available.
4. Hide the launcher, choose **Open Launcher**, and confirm the window reopens
   with its search input focused.
5. Choose **Rebuild Index** and confirm rebuild progress appears when the
   launcher is opened.
6. Enable **Launch at Login**. Confirm it becomes checked and appears in
   **System Settings → General → Login Items & Extensions**. If macOS requires
   approval, approve it there.
7. Disable **Launch at Login** again and confirm it becomes unchecked. The
   current process should continue running.
8. While the app is running, execute its bundled binary directly:

   ```sh
   "dist/Markdown Launcher.app/Contents/MacOS/markdown-snippet-manager"
   ```

   Confirm it reports that Markdown Launcher is already running and exits.
9. Choose **Quit Markdown Launcher** and confirm the menu bar item disappears.
10. Open the application again manually and confirm it starts normally.

To test automatic startup, enable **Launch at Login**, log out and back in, and
confirm the menu bar item appears without the search window opening. Use the
global shortcut or **Open Launcher** to show it, then disable the option if it
should not remain enabled.

### Settings

1. Choose **Settings…** from the macOS menu bar item.
2. Confirm the search popup hides and Settings is not obscured by it.
3. Confirm the configured scan root, other current values, and recommended
   defaults are displayed.
4. Choose **Restore Defaults** and confirm the fields change without saving;
   close and reopen Settings to recover the previously saved values.
5. Add a controlled folder with **Add Folder…**, remove an unwanted root, and
   change another field such as the extensions list.
6. Choose **Save Settings** and confirm the restart-required message appears.
7. Enter an invalid value, such as `0` for the maximum file size, and confirm
   saving reports an error without replacing the last valid configuration.
8. Close and reopen Settings and confirm the last valid saved values remain and
   the search popup stays hidden.
9. Reopen search with the global shortcut.
10. Quit and reopen Markdown Launcher and confirm the saved roots and shortcut
    take effect.
11. Open Settings, remove its last root from the draft, and choose **Add Folder...**.
    Leave the native folder sheet open for at least 10 seconds while background
    status polling continues. The app must remain running and the sheet must
    stay above Settings. Cancel: the draft should remain empty, saving should
    report an error, and the saved configuration must remain unchanged.
12. Open the picker again, select a replacement root, and verify it appears in
    the draft. Repeat opening/cancelling the picker several times. Nothing is
    persisted until **Save Settings** succeeds. Close and reopen without saving
    to recover the original roots if these were only test changes.

### New notes and destination safety

Use the controlled fixture root rather than creating test notes in important
project folders. Save changed scan settings and restart before these checks;
creating notes within those active roots does not require another restart.

1. Search for `Creation test`, press Command/Ctrl+N, and confirm the dialog
   suggests `Creation test.md` with black text on a white input. No file should
   exist yet. Escape must restore the original query and selected result.
2. Open New Note again. Choose a folder under the fixture root. Confirm the
   native folder sheet appears above the launcher, can be cancelled, and returns
   focus to the dialog rather than hiding the app.
3. Confirm creation. Verify the dialog closes, search clears, and the new note
   is selected with its source editor focused after the heading. Check the source
   contains `# Creation test` followed by a newline. Existing files must be unchanged.
4. Search again and confirm the new note becomes searchable without a restart
   or rebuilding the entire index.
5. Open New Note and enter the same filename/destination. Confirm an inline
   existing-name error leaves both values intact; verify source contents did
   not change. Correct the name and retry successfully.
6. Try a filename with a separator, an empty filename, a trailing space, and an
   unsupported extension. Each must report an error without creating a file.
7. Choose a folder outside every configured root, then try Create Note. Confirm
   the root-boundary error stays in the dialog and no source is created there.
8. Repeat with `node_modules` and a folder excluded by a root-relative glob.
   Nested folders beneath an excluded ancestor must also be rejected.
9. With symlinks disabled, try a symlink destination. With symlinks enabled,
   a link resolving outside every root must still be rejected.
10. Try a folder without write permission. Confirm the error stays in the dialog
    and the filename/destination remain available for correction.
11. Create in a different eligible subfolder, restart, and open New Note again.
    Confirm the last successful folder is remembered and scan settings are
    unchanged. Cancel should not change the remembered destination.
12. Open the dialog from a focused preview, then cancel. Preview focus, query,
    and selection should be restored. Whole-note copy and result navigation
    must not run behind the dialog.
13. Type into the new note, wait for Saved, and verify its source and searchable
    body update. Open New Note from an existing editor, cancel, and confirm its
    buffer and focus are restored.

### Source editing, autosave, and accepted loss

Use only disposable fixture notes for failure tests; failed saves after leaving
intentionally lose changes. These native checks remain separate from unit tests.

1. Select a fixture created outside the app and press Command/Ctrl+E. Confirm the
   complete source, including frontmatter, appears in a readable multiline input.
   Check the blue caret against the dark editor, then check its contrast in white
   Settings/New Note inputs. Verify selection, scrolling, long lines, and Unicode.
2. Modify only body text, pause at least 500 ms, and confirm Unsaved -> Saving ->
   Saved and the full source on disk. Frontmatter and untouched newlines must remain.
   Edit again and press Command/Ctrl+S before the debounce expires.
3. Verify arrows, Enter, Command/Ctrl+A/C, undo/redo, and clipboard paste act on
   the source input without navigating results or hiding. Whole-note
   Command/Ctrl+Shift+C must do nothing while an editor is open.
4. Press Escape once: editing ends, changes are submitted, and read-only preview
   appears with search focused. The query and selected result remain. Typing must
   update search immediately; Down browses results. Command/Ctrl+E reopens editing.
   Rapid Up/Down browsing must not modify untouched files.
5. Type and immediately hide via two Escapes, focus loss, or window close.
   Reopen and confirm the last changed source saved; no draft is restored. Repeat
   with the app's Quit action and relaunch. Check the preview reloads saved content.
6. Make only a fixture's containing directory unwritable, leaving the file readable
   and writable. Edit it and wait for the explicit save failure. Restore directory
   permissions without changing the source file, choose Retry Save, and confirm
   the latest buffer saves. File-permission changes may themselves cause a conflict.
7. Repeat the failure but switch notes or hide before retrying. Reopen and confirm
   the reason says changes were discarded, the source stayed unchanged, and there
   is no recovered draft or retry for that lost content. Another note must remain editable.
8. Schedule an external file update while the editor stays focused, then edit and
   save. Expect a conflict, not an overwrite. Leave and reopen to load the external
   version. Also test deletion, replacement, and a read-only source.
9. While editing, let indexing refresh the same query. It must not replace the
   live buffer, including when saved edits remove the original search match.
10. Quit with pending changes in a disposable note. Confirm a save is attempted
    and the app exits even on failure. Do not expect recovery after force-quit or
    a crash. macOS permission dialogs and native shutdown timing need real testing.

### Workspace buttons, focus, and long content

These are native acceptance checks, not claims established by headless tests.
Use disposable fixture notes and leave the installed scan configuration unchanged.

1. Confirm **New** is beside search. Click it with a query and with an empty query;
   check the filename suggestion, cancel, and confirm query/selection are unchanged.
2. Hover different rows without clicking. Edit/Copy should appear only on hovered
   or keyboard-selected rows; titles, paths, and excerpts must not shift.
3. Select Alpha and wait for preview. Hover Beta and click its disabled Copy icon.
   Alpha must remain selected, with no clipboard change or hiding. The tooltip
   must explain why Beta cannot yet be copied.
4. Click Beta's Edit icon. Beta must become selected and its source must receive
   keyboard focus immediately after loading. Typing must reach Beta, not search;
   Alpha must remain unchanged. Escape must show preview and focus search.
5. During editing, check that every Copy icon and Command/Ctrl+Shift+C are disabled.
   Open New and cancel from editor focus, then repeat from search focus while the
   editor remains open. Preserve the buffer and restore the respective focus.
6. Select a ready preview, change its fixture on disk, then use its Copy icon.
   The real clipboard must receive the current body without frontmatter, and the
   popup must hide only after success. Test a missing/unreadable file too: keep
   query/selection and show a readable reason without hiding.
7. Use Tab from search to reach New and the selected row's enabled actions. Space
   should activate them; invisible or disabled actions must not receive focus.
8. Inspect normal, hover, pressed, and keyboard-focus button states. Check caret
   and selection contrast in source, preview, search, and the white New dialog.
9. Use a very long filename and long save failure. Check the filename fits its pane
   and its tooltip shows the full path. Failure details must wrap and scroll while
   mode, save status, source, and Retry Save remain reachable. Retry should focus
   source again and submit the latest buffer.
10. Check action labels and unavailable reasons with VoiceOver. GPUI currently
    omits the native disabled accessibility flag, so verify descriptive wording
    as well as blocked activation. Keep native visual/accessibility acceptance
    separate from automated interaction and layout coverage.

### Current-file clipboard behavior

1. Search for `Alpha snippet`.
2. Modify `alpha.md` after its result is visible.
3. Reopen the launcher with the global shortcut if changing focus hid it.
4. Select Alpha using Down or a click, wait for preview, then press
   Command/Ctrl+Shift+C.
5. Read the real system clipboard.

On macOS:

```sh
pbpaste
```

The clipboard must contain the latest body and no YAML frontmatter.

### Explicit selection and preview

1. Open the launcher and search for a known document.
2. Confirm no result is selected and the preview asks you to select a note.
3. Press Enter and confirm nothing is copied or hidden.
4. Press Down and confirm the first result is selected and previewed.
5. Change the query and confirm selection and preview clear immediately.
6. Press Up and confirm the last matching result is selected and previewed.
7. Click another result; confirm its preview appears without changing the clipboard.
8. Navigate quickly through several results; confirm the final preview always
   belongs to the selected path, never an earlier result.
9. Modify the selected file externally, reopen the launcher if necessary, and
   confirm the preview refreshes after filesystem synchronization.
10. Delete the selected file and confirm selection clears after synchronization.
11. Use a note below the configured file-size limit but above 24,000 characters
    and 240 lines; confirm scrolling reaches its final paragraph with no truncation
    notice. Repeat with a large code fence and a single very long paragraph.
    Confirm Command/Ctrl+Shift+C copies the full original body.
12. Scroll a long preview, select a different document, and confirm its preview
    starts at the top. Confirm unrelated status updates do not reset scrolling.
13. Confirm embedded image URLs do not cause downloads and preview never writes
    to the Markdown files.

### Preview focus and text copying

1. Search for a note, explicitly select it, and wait for preview to load.
2. Press Enter. Confirm focus moves to preview without copying or hiding.
3. Use Up/Down and Page Up/Down; confirm preview scrolls without changing results.
4. Drag across part of a paragraph and press Command/Ctrl+C. Confirm only that
   rendered text is copied and the workspace stays open.
5. Start selecting near the top of a long preview, scroll while dragging or hold
   the pointer below its viewport, and finish near the bottom. Confirm copying
   includes the intervening offscreen text, with no missing virtual rows.
6. Press Command/Ctrl+A, then Command/Ctrl+C; confirm the entire rendered document
   is copied, including offscreen content.
7. Press Escape; confirm focus returns to search with query and result preserved.
8. Use Command/Ctrl+A and Command/Ctrl+C in search; confirm only search text is copied.
9. Press Command/Ctrl+Shift+C; confirm the latest Markdown body is copied and the
   workspace hides, even if only a substring was selected in preview.
10. Repeat whole-note copying with no selection, while loading, and after a
    preview error; confirm it is disabled and leaves the clipboard unchanged.

### Escape behavior

On macOS, set a known clipboard value:

```sh
printf 'clipboard must remain unchanged' | pbcopy
```

Open the launcher, press Escape, and verify `pbpaste` still returns the marker.

### Create, modify, rename, and delete

Run each command separately and allow approximately one second for watcher
debouncing:

```sh
echo "unique newly created content" > "$TEST_ROOT/created.md"
echo "unique modified content" > "$TEST_ROOT/created.md"
mv "$TEST_ROOT/created.md" "$TEST_ROOT/renamed.md"
rm "$TEST_ROOT/renamed.md"
```

Verify search adds, updates, renames, and removes the result after each step.

### Malformed file isolation

```sh
cat > "$TEST_ROOT/invalid.md" <<'EOF'
---
title: Missing closing delimiter
invalid body
EOF
```

The application should stay running, report an issue, and omit the file from
search. Fixing the frontmatter should make the file searchable after the next
watcher update.

### Oversized file isolation

On macOS or Linux:

```sh
dd if=/dev/zero of="$TEST_ROOT/oversized.md" bs=1m count=3
```

With the default limit, the file should be skipped and reported without
crashing the application.

### Manual rebuild

Press Command/Ctrl+Shift+R while the launcher is visible. The footer should
report rebuild progress, results should return, and source files must remain
unchanged.

### Corrupt-index recovery

1. Stop the application.
2. Back up the application-data directory if it contains important local state.
3. Remove the index `-wal` and `-shm` sidecars.
4. Replace `index.sqlite3` with non-SQLite content.
5. Restart the application.

The launcher should start, quarantine the unusable database as
`index.sqlite3.corrupt-*`, and rebuild searchable results from the configured
roots.

## Current product boundaries

The current release does not provide:

- Deleting Markdown files
- On-disk draft checkpoints or crash recovery
- Full-fidelity Markdown rendering or code syntax highlighting
- Draft-aware whole-note copying
- Rich-text clipboard output
- Cloud synchronization
- A Developer ID-signed or notarized distribution package

Markdown files remain the source of truth. Explicit editing can update eligible
sources; indexing, searching, previewing, and rebuilding never modify them.

## Cleanup after controlled testing

Stop the application before cleanup. Remove the fixture directory when it is no
longer needed:

```sh
rm -rf "$TEST_ROOT"
```

Restore any configuration or application-data backup created before testing.
Do not restore or replace the SQLite database while the application is still
running.
