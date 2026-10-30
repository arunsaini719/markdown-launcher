# Contributing

Thank you for helping improve Markdown Launcher.

## Before starting

For a significant behavior change, open an issue first and describe the user
problem, the proposed interaction, and any platform-specific constraints. Small
bug fixes and documentation corrections can go directly to a pull request.

Read these documents before changing core behavior:

- [Product scope](docs/product-scope.md)
- [Architecture](docs/architecture.md)
- [Manual setup and native testing](manual_setup.md)

## Development setup

Install Rust 1.99 or later and the native dependencies from the
[GPUI Kit installation guide](https://gpui-kit.com/docs/installation). The
checked-in toolchain file selects the expected Rust version and components.

Run the application during development with:

```sh
cargo run
```

The launcher reads and writes real Markdown files. Use a temporary directory as
a scan root when testing creation, editing, exclusion, or synchronization
behavior manually.

## Quality checks

Run the same checks used in CI before opening a pull request:

```sh
./scripts/check.sh
```

The script checks formatting, runs Clippy with warnings denied, and executes all
targets with every feature enabled.

Behavior changes should include focused tests. Filesystem behavior must cover
failure and race conditions where practical, especially around symlinks,
external modifications, atomic writes, and index reconciliation.

## Pull requests

Keep each pull request centered on one user problem. Explain why the change is
needed, describe observable behavior, and call out platform-specific testing.
Do not commit `target/`, `dist/`, local databases, or generated application
bundles.

## Licensing

Unless explicitly stated otherwise, contributions are submitted under the same
MIT OR Apache-2.0 terms as the project.
