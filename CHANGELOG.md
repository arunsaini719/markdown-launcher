# Changelog

Notable user-facing changes to Markdown Launcher are recorded here. The project
has not published a versioned release yet.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and intends to use [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Local Markdown discovery across configurable roots and exclusions
- SQLite FTS5 search over titles, tags, paths, and document bodies
- Keyboard and pointer result navigation with a virtualized read-only preview
- Root-constrained note creation with collision-safe atomic publication
- Explicit source editing with autosave and external-change detection
- Native macOS menu bar controls, settings, Launch at Login, and app packaging
- Tag-triggered, architecture-specific macOS prerelease archives with checksums

### Security

- Root, exclusion, extension, size, permission, and symlink checks before writes
- Conflict-aware atomic replacement that refuses external modifications
- Local-only preview rendering that never fetches linked remote resources

### Known limitations

- macOS archives are ad-hoc signed and are not Developer ID signed or notarized
- Linux native global shortcuts require an X11 session
- Deletion, rich inline Markdown rendering, and syntax highlighting are deferred
