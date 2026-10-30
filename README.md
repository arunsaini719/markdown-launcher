# Markdown Launcher

[![CI](https://github.com/arunsaini719/markdown-launcher/actions/workflows/ci.yml/badge.svg)](https://github.com/arunsaini719/markdown-launcher/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A local-first, keyboard-driven desktop launcher for finding, previewing, and
editing Markdown notes without interrupting your workflow.

Markdown Launcher recursively discovers files under explicitly configured
roots and maintains a rebuildable SQLite FTS5 search index. Markdown files stay
the source of truth, search and editing work without a network service, and the
index can be discarded and recreated at any time.

## Why Markdown Launcher?

- Find notes by title, tags, path, or full Markdown content.
- Keep notes as portable files instead of storing them in an application silo.
- Navigate, preview, copy, create, and edit without leaving the keyboard.
- Detect external changes before atomically replacing edited files.
- Run indexing and filesystem synchronization entirely on the local machine.

## Current scope

- Discover `.md` and `.markdown` files under the configured roots
- Default the initial root to the current user's home directory
- Search titles, tags, and Markdown content with SQLite FTS5
- Select results explicitly with the keyboard or mouse
- Preview the selected document in a read-only right-hand pane
- Copy the selected document body and hide the launcher
- Create new Markdown notes in eligible folders inside the configured roots
- Explicitly edit eligible Markdown files with in-memory drafts and autosave
- Keep the index synchronized with filesystem changes
- Operate locally without a network service

New-note creation is explicit and never overwrites existing files. Previewing is
read-only; explicit editing is available for discovered and newly created notes.
Deletion remains deferred.

## Documentation

- [Product scope](docs/product-scope.md)
- [Architecture](docs/architecture.md)
- [Manual setup and native testing](manual_setup.md)

## Launcher controls

The default global shortcut is `Command/Ctrl+Shift+Space`. It can be changed
through the `global_shortcut` value in the generated configuration file.

- Type to search titles, tags, and Markdown bodies.
- Results start unselected, including after a query change.
- Use Down to select the first result, Up to select the last, or click a result.
  Selection loads a read-only preview; clicking does not copy or hide.
- Press Ctrl+Home/End to jump to the first or last result.
- Press Enter on a selected, loaded document to focus its read-only preview.
- In preview, Up/Down and Page Up/Down scroll; Home/End (or Command+Up/Down)
  jump to the beginning/end. These keys do not change the selected result.
- Press Command/Ctrl+Shift+C to copy the selected document's current Markdown
  body and hide the launcher. This requires a loaded, error-free preview.
- Drag preview text to select it; double-click selects a word and Shift-click
  extends a selection. Command/Ctrl+A selects the full rendered document,
  including offscreen text; Command/Ctrl+C copies only the selected plain text
  without hiding. In search, these shortcuts retain normal input behavior.
- Press Command/Ctrl+E to edit the selected file's complete Markdown source.
  Editor arrows move the cursor, not the result selection. Command/Ctrl+S saves
  immediately; otherwise autosave starts after 500 ms of inactivity.
- Hover over a result to reveal compact **Edit** and **Copy** icon buttons; they
  also stay visible on the keyboard-selected row without moving its text.
  Edit selects that row and opens its source. Copy never selects another row:
  it requires that row's ready preview and is disabled during editing or copying.
- Escape from the editor submits changes, shows read-only preview, and focuses
  search with query and result preserved. Escape from preview also focuses search.
  Escape from search resets the query and hides, canceling pending copy work.
- Click **New** beside search or press Command/Ctrl+N to open the New Note dialog.
  Review the full filename and destination, then press Enter or choose **Create Note**. Escape cancels without
  changing the current query or selection. Validation errors remain in the dialog.
- Press Command/Ctrl+Shift+R to rebuild the disposable search index.
- On macOS, press Command+, or choose **Settings...** from the menu bar to hide
  the search popup and edit indexing and shortcut settings. The window shows the
  recommended defaults and can restore them to the unsaved draft. Saved changes
  apply after restarting the app.
- On macOS, press Command+Q or choose **Quit Markdown Launcher** from the menu
  bar item to stop the resident application.

The application remains resident while the launcher is hidden so indexing and
filesystem synchronization can continue. On macOS, its menu bar item provides
**Open Launcher**, **Settings...**, **Rebuild Index**, **Launch at Login**, and
**Quit Markdown Launcher** actions. Launch at Login is disabled by default and
can be toggled explicitly after installing the app. Only one macOS application
instance runs at a time.

Results always show the file path separately from the title and content excerpt.
Paths under the home directory use `~/`; hover over result text or the document
pane's filename to see the full path. The pane identifies preview/editing mode;
editing also shows save status above the source. Long failure details wrap and
scroll without hiding the editor's Retry Save button.

The complete document is read and parsed in the background after a 100 ms
selection debounce, within the configured file-size limit (2 MiB by default).
The preview is not truncated: a variable-height virtual list lays out visible
rows plus a small buffer, so scrolling reaches the end of the document. Long
paragraphs and code blocks are split into bounded display rows without losing
text. The full parsed preview remains in memory; this is not progressive file
loading. Raising the file-size limit increases loading time and memory use.
Copy still reads the full current body. Basic headings, paragraphs, lists, and
code blocks are supported; syntax highlighting and rich inline formatting are
deferred. Links and images never load remote content. Preview selection is
anchored to the full rendered text, so unmounted rows remain part of a selection.
Selecting another document or reloading a preview clears its text selection.

## New notes

The creation dialog opens every time; folder setup is not repeated. The search
query supplies an editable filename, or `Untitled.md` for an empty query. The
suggestion uses a configured extension; the entered filename is never silently
renamed or given a collision suffix.

The last successful destination is remembered across launches. On first use,
the app suggests `Documents/Markdown Launcher Notes` only if it is eligible
under an active scan root; otherwise it suggests an eligible configured root.
Use **Choose...** to select any eligible folder inside the roots. Outside-root,
excluded, and disallowed symlink destinations are rejected. Missing subfolders
are created only after confirmation. macOS may request Documents access.

Existing names, invalid filenames/extensions, permission errors, and filesystem
failures appear inline without discarding the draft. The complete note is
published atomically without overwriting another file, including a concurrent
creator. A new note initially contains a heading derived from its filename.
After success, the search clears and the new note opens in the editor, with the
cursor after its heading. It remains in that result set while indexing catches
up, even beyond the normal result limit. Changing the query clears this temporary
inclusion. Creation does not require a restart or change scan settings.

## Editing and saving

Command/Ctrl+E explicitly edits any eligible viewable Markdown file, not just
app-created notes. Active roots, exclusions, extensions, size limits, symlink
policy, and filesystem permissions still apply. Editing uses the complete source,
including frontmatter, rather than reconstructing it from the preview.

There is one active in-memory draft. Changed content is saved asynchronously with
serialized atomic replacement and external-change checks. The editor shows
**Unsaved**, **Saving**, **Saved**, or a failure reason with **Retry Save**.
Retry submits the latest buffer while it remains open. A conflict never offers
force-overwrite: copy anything needed, leave the editor, then reopen the disk version.

Leaving editing with Escape, switching documents, changing the query, or hiding
submits changed content and
releases the draft immediately. A failed background save discards its submitted
bytes too; only a failure message remains. It does not block subsequent editing
or retain a retryable detached draft. Browsing previews does not write files.
Quit attempts outstanding saves and exits even on failure; native system shutdown
is best effort within GPUI's shutdown timeout. There are no draft checkpoints or
crash recovery, so unsaved edits may be lost. Atomic save temporary files are not
recovery drafts.

Ordinary editor select-all/copy uses the source buffer and does not hide. The
whole-note Command/Ctrl+Shift+C action is disabled while an editor is open;
draft-aware whole-note copying remains deferred.

## Reliability

The launcher reports live indexing counts and bounded file-level diagnostics.
Filesystem watchers reconnect automatically and perform a reconciliation after
reconnecting. Dropped-event rescan notifications also trigger a full
reconciliation. Directory events scan only the affected subtree.

Full reconciliation removes cached documents from roots removed from the
configuration without modifying their source files. Scope cleanup is deferred
when any configured root cannot be resolved, rather than treating an unresolved
root as an empty indexing scope.

If the disposable SQLite index fails its integrity or expected-table checks,
the application moves it aside with a `.corrupt-*` suffix and rebuilds a clean
index from the Markdown sources. An invalid shortcut uses the default shortcut
without changing scan settings. Malformed configuration or invalid scan settings
stop startup rather than widening indexing scope; the invalid file is preserved.

Background scans support cancellation, and shutdown waits for the index worker
to finish its current transactional write before exiting.

## Project status

The search-and-copy launcher workflow, explicit-selection read-only preview,
root-constrained note creation, explicit source editing and autosave, native
settings, local macOS `.app` packaging, menu bar controls, opt-in login startup,
and single-instance protection are implemented.
Developer ID signing, notarization, installers, and public release automation
remain future work.

## Platform support

macOS 15 or later is the primary supported desktop platform and includes local
`.app` packaging, menu bar controls, Launch at Login, and single-instance
protection. The core application also builds on Linux; native global shortcuts
require an X11 session. Signed and notarized release binaries are not available
yet.

## Development

The application is built with Rust and
[GPUI Kit](https://gpui-kit.com/docs/getting-started), which uses GPUI for native
windowing and rendering.

Prerequisites:

- Rust 1.99 or later; the checked-in toolchain file selects the required version
- The native dependencies listed in the
  [GPUI Kit installation guide](https://gpui-kit.com/docs/installation)
- On macOS, macOS 15 or later and the Xcode Command Line Tools

Run the development application:

```sh
cargo run
```

Inspect the binary version or command-line help without starting the desktop
application:

```sh
cargo run -- --version
cargo run -- --help
```

Build a local, ad-hoc-signed macOS application:

```sh
./scripts/build-macos-app.sh
open "dist/Markdown Launcher.app"
```

Run the local quality checks:

```sh
./scripts/check.sh
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow, testing
expectations, and pull request guidance.

## License

Markdown Launcher is available under either of the following licenses, at your
option:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)
