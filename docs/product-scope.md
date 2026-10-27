# Product Scope

## Purpose

The application is a keyboard-driven desktop launcher for Markdown content. It
lets a user open a small search window, find a Markdown document, copy its body
to the clipboard, and return to their previous application.

The product supports local search, preview, copy, root-constrained creation, and
explicit source editing with autosave. It does not delete files.

## Primary workflow

1. The application starts and remains available in the background.
2. It discovers supported Markdown files below the configured scan roots.
3. The user opens the launcher with a global keyboard shortcut.
4. Typing searches indexed titles, tags, and document content.
5. Results start unselected. Up, Down, or a click explicitly selects a result
   and loads a read-only preview in the right pane. Changing the query clears
   selection and preview; hover never selects.
6. Enter on a selected, loaded document focuses the read-only preview. Arrow
   and paging keys scroll it without changing the selected search result.
7. Command/Ctrl+Shift+C reads the selected file, copies its current Markdown
   body, and hides the workspace. No selection, loading, and preview errors
   disable whole-note copying, as does an open editor.
8. Escape from editing submits changes, shows read-only preview, and focuses
   search without losing the query or result. Escape from preview also focuses
   search. Escape from search hides the launcher without changing the clipboard.
9. Command/Ctrl+E explicitly enters source editing. Editor arrows move the cursor;
   search arrows browse results. Command/Ctrl+S saves immediately; autosave waits
   500 ms after typing stops. New notes open directly in the editor.

Selecting or previewing a note does not hide the workspace. Whole-note copying
hides it without terminating the background application.

## Files and scan roots

The default scan root is the current user's home directory, resolved through
the operating system rather than a hardcoded path. All descendant directories
are eligible for scanning, subject to the safety rules below.

The initial supported extensions are:

- `.md`
- `.markdown`

Extension matching is case-insensitive. Other file types are ignored.

The scanner does not follow symbolic links by default. This prevents cycles and
avoids unexpectedly crossing outside a configured root.

Initial default exclusions include common generated or internal directories:

- `.git`, `.hg`, and `.svn`
- `node_modules`
- `target`
- `.cache`
- `.Trash`
- platform cache directories such as `Library/Caches`

Scan roots, exclusions, supported extensions, and maximum indexed file size are
configuration values exposed through the native settings window and the TOML
file. This allows the safe defaults to be changed without redesigning the
indexer.

Unreadable directories, unsupported encodings, malformed documents, and files
above the configured size limit are skipped and reported without stopping the
scan.

## Markdown metadata

A document may provide YAML frontmatter:

```markdown
---
id: 9d994257-7f61-4b18-90d1-63acc03f8c34
title: Tokio main function
tags:
  - rust
  - async
---

#[tokio::main]
async fn main() {}
```

Metadata rules:

- `title` is optional and falls back to the filename without its extension.
- `tags` is optional and defaults to an empty list.
- `id` is optional for discovered files.
- The canonical file path is the identity used by the initial search index.
- Frontmatter is indexed as metadata but is not copied with the document body.

Files without frontmatter remain valid and searchable.

## Search behavior

Search is local and uses SQLite FTS5. It searches:

1. Title
2. Tags
3. Markdown body

Title and tag matches should rank above body-only matches. Empty queries may
show a bounded list of recently selected or consistently ordered documents.
Exact ranking and recent-use behavior can evolve without changing file storage.

The SQLite database is a derived index, not the source of truth. Deleting or
rebuilding it must not modify Markdown files.

## Index lifecycle

On the first launch, the application scans the configured roots in the
background and adds results in batches. The UI remains responsive and exposes
scan progress.

On later launches, existing SQLite results are available immediately while a
background reconciliation checks for additions, changes, renames, and
removals. A filesystem watcher applies subsequent updates. A manual rebuild is
available as a recovery operation.

## Privacy and safety

- All indexing and searching happen on the local machine.
- No document content is sent over the network.
- The scanner stays within configured roots unless the user adds another root.
- The application does not follow symbolic links by default.
- Previewing never modifies files; only explicit editing permits source updates.
- Editing revalidates eligibility and permissions and checks for external changes.
- Failed saves after leaving an editor discard new edits; no recovery is promised.
- Copy reads and reparses the selected file so the clipboard receives its
  current body rather than a stale indexed version.

## Current release boundaries

Included:

- Configurable recursive Markdown discovery
- Persistent SQLite metadata and full-text index
- Incremental filesystem synchronization
- Keyboard-driven GPUI Kit launcher
- Clipboard copy
- Explicit, non-overwriting note creation with per-note filename/destination confirmation
- Complete-source editing, serialized atomic autosave, and visible save failures/retry
- New beside search, with compact Edit/Copy actions on hovered or selected result rows
- Virtualized full-document read-only Markdown preview with explicit selection
- Resident application and global shortcut
- macOS menu bar controls, opt-in login startup, and single-instance protection
- Native settings with validated, restart-required configuration changes
- Index status, errors, and rebuild support

Not included:

- Deletion of Markdown files
- On-disk draft checkpoints or crash recovery
- Draft-aware whole-note copying
- Cloud synchronization
- Team sharing or authentication
- Search of non-Markdown files
- Rich-text clipboard formats
- Full-fidelity Markdown rendering and code syntax highlighting
- Mobile or web clients

## Read-only workspace stage

The launcher uses a roughly 35/65 results/preview split. No document is loaded
until the user selects one. Down starts at the first result and Up at the last;
navigation stops at list boundaries. Clicking selects and previews without
copying or hiding. Background index refreshes preserve only an explicitly
selected document that still matches, and reload its preview from disk.

Preview loading is debounced and asynchronous. A stale success or failure must
never replace a newer selection. The entire file is read and parsed within the
configured file-size limit. The complete preview is scrollable without
truncation; only visible rows plus a small buffer are laid out. Long blocks are
split into bounded display rows, including UTF-8 text and large code fences.
Virtualization reduces layout work, not the initial file read or full-document
memory use. Missing, invalid, and oversized files report preview errors without
changing the clipboard or source file. Frontmatter is not previewed. Links and
images are represented as text, never fetched or opened.

Basic block formatting and text selection are supported. Drag to select,
double-click a word, or Shift-click to extend. Normal copy copies the selected
rendered plain text without hiding, while select-all includes the complete
rendered document even when most rows are unmounted. These shortcuts remain
native to the search input when it has focus. Text selection clears when the
preview document is replaced, preventing copying from a previous note.

Rich inline styling and syntax highlighting remain deferred.

## New-note creation stage

Command/Ctrl+N opens a creation dialog every time. The search query supplies a
suggested full filename; an empty query supplies `Untitled` with a configured
extension. Users may edit the filename and choose a destination. Enter confirms,
Escape cancels, and errors remain inline with the draft intact. Existing files
are never overwritten or silently renamed.

Any eligible destination inside an active scan root is allowed; a separate
managed-notes folder is not required. Canonical root boundaries, directory and
glob exclusions, supported extensions, and the symlink policy apply. Filenames
must be single names without separators, control characters, leading/trailing
spaces, or a trailing dot. Initial content must fit the configured size
limit. Writes use anchored directory handles to reject symlink swaps, and
atomic non-overwriting publication protects against racing creators.

The last successful destination is remembered separately from configuration.
The first-use suggestion is the OS Documents folder's `Markdown Launcher Notes`
subfolder, but only if eligible; otherwise use an eligible configured root.
No folder or note is created until confirmation. Creating a note does not
require restarting or broaden scan scope.

New notes contain a Markdown heading and use the filename as their indexed
title. Successful creation clears the search, explicitly selects the document,
and opens its source editor with the cursor after the heading. This explicit
action is an exception to unselected search results. The new note remains included in that result set until the query
changes or the file disappears, allowing indexing to catch up and keeping
notes beyond the result limit reachable. Cancel restores search, preview, or
editor focus according to the originating context without changing query or selection. Workspace shortcuts cannot act
behind the dialog; confirmation disables further creation/cancellation until
its write completes.

## Editing and autosave stage

Any viewable eligible Markdown file can be edited explicitly, regardless of
which application created it. There is no ownership registry or managed-folder
requirement. Active roots, exclusions, extensions, size limits, symlink policy,
and OS permissions apply to loading and saving. Editing retains the complete
original source, including frontmatter, BOM, and existing newlines.

Only one active draft is kept in memory. Autosave follows 500 ms of inactivity;
Command/Ctrl+S saves immediately. Writes are serialized, revision-aware, and
coalesce newer pending snapshots. Unchanged content and ordinary preview browsing
do not write. Same-query index refreshes preserve the live editor, even if edits
remove its original search match or the source disappears. Successful saves request
incremental indexing
through the existing single-writer boundary.

The editor shows Unsaved, Saving, Saved, or Save failed with its reason. While
open, Retry Save submits the latest buffer. Conflicts require leaving and
reopening the disk version rather than force-overwriting another writer.
Leaving editing with Escape, switching documents, changing the query, or hiding
submits changed content and
immediately releases the draft. The save task owns its submitted bytes only until
completion, then discards them on failure as well as success. Failed detached
saves retain only a message and do not block another editor or offer content retry.
Other files remain editable while submissions are pending. Only reopening the
same file waits for its earlier writes; failed drafts are never retained afterward.

Quit attempts outstanding saves, then exits even on failure. Native system quit
makes a final attempt within the framework's shutdown timeout. Crashes lose
unsaved memory. No disk draft checkpoints, recovery registry, or crash recovery
are provided. Ordinary editor copy/select-all follows the source input; whole-note
copy remains disabled during editing until draft-aware copying is implemented.
Canceling New Note preserves a current editor buffer and restores the originating
search/editor focus. Deletion remains deferred.

## Workspace controls and polish

**New** beside search opens the same per-note confirmation dialog as Command/Ctrl+N.
Compact Edit and Copy icon buttons appear on hovered or keyboard-selected result
rows. Their space stays reserved, so hovering never moves titles or selects a note.
Edit explicitly selects its own row and opens source editing, submitting any prior
draft through the existing best-effort transition. Copy requires that same row to
already be selected with a ready preview. It never selects another row to enable
copying, and remains disabled during editing or an outstanding copy.

Action clicks do not also trigger row selection. Pending or stale result actions
cannot edit or copy a different file. Tooltips explain shortcuts and unavailable
states; only selected, enabled row actions join keyboard tab navigation. Result
arrows return focus to search before an old row action can become invisible, and
removing a focused control restores an available workspace input.

The document pane shows mode and filename, with the full path available on hover.
The editor shows save status above the source and wraps long failure details in a
bounded scrolling region while keeping Retry Save visible. Workspace buttons keep
readable normal, hover, and active colors independently of the light dialog theme.
These controls do not change source authority, save policy, or copy semantics.

## Initial acceptance criteria

The initial release is complete when:

- A first-run background scan discovers eligible Markdown files under the home
  directory without blocking the window.
- A later launch can search the existing index before reconciliation finishes.
- Title, tag, and body queries return ranked results.
- Filesystem additions, updates, renames, and removals update the index.
- Results remain unselected until Up, Down, or click; query changes clear the
  selected document and preview.
- Preview loading enforces the file-size limit and rejects obsolete completions.
- The full preview is reachable by scrolling without laying out every row.
- Enter focuses a ready preview; Escape returns to search before hiding.
- Command/Ctrl+Shift+C copies the latest Markdown body and hides the launcher.
- Normal copy/select-all follow focus and include selected offscreen preview text.
- New-note confirmation accepts only eligible root-constrained destinations,
  never overwrites a name, and reports errors without losing the dialog draft.
- Cancel creates nothing; successful creation opens the note in the editor
  without a restart or modifying another note.
- New and row Edit/Copy controls preserve explicit selection, modal isolation,
  focus, and disabled-copy rules without moving result text on hover.
- Explicit editing preserves source outside intentional changes, autosaves safely,
  reports failure reasons, and retries only while the active draft remains.
- Switching/hiding never retains failed detached drafts, and crashes have no recovery.
- Invalid files, unavailable paths, and database recovery do not crash the
  application.
- The application performs no network requests for indexing or search.
