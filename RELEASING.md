# Release Process

Markdown Launcher currently publishes ad-hoc-signed macOS builds as GitHub
prereleases. Do not present these archives as notarized production installers.

## 1. Prepare the version

Start from a clean, current `main` branch with a successful CI run.

1. Update `version` in `Cargo.toml` and run `cargo check` so the workspace
   package entry in `Cargo.lock` records the same version.
2. Move the relevant entries in `CHANGELOG.md` from `Unreleased` into a heading
   such as `## [0.1.0] - 2026-10-27`.
3. Leave a new empty `Unreleased` heading for subsequent work.
4. Run the complete local checks and package verification:

   ```sh
   ./scripts/check.sh
   ./scripts/package-macos-release.sh
   ```

5. Confirm the packaged binary and checksum:

   ```sh
   ./scripts/verify-macos-app.sh "dist/Markdown Launcher.app"
   cd dist
   shasum -a 256 -c markdown-launcher-v*-macos-*.zip.sha256
   ```

Commit the version and changelog together with a message such as
`Prepare v0.1.0` and wait for CI to pass.

## 2. Create the release tag

The workflow requires an existing `v`-prefixed semantic version tag that exactly
matches the Cargo package version.

```sh
git switch main
git pull --ff-only
git tag -a v0.1.0 -m "Markdown Launcher v0.1.0"
git push origin main
git push origin v0.1.0
```

Pushing the tag starts `.github/workflows/release.yml`. The workflow:

1. Verifies the tag against `Cargo.toml`.
2. Runs formatting, Clippy, and all tests.
3. Builds and ad-hoc signs the macOS application.
4. Builds separate Apple Silicon (`arm64`) and Intel (`x86_64`) application
   archives on native GitHub-hosted runners.
5. Verifies both SHA-256 checksums after downloading the isolated build outputs.
6. Records signed GitHub build-provenance attestations for both archives.
7. Publishes both archives and checksums as a single GitHub prerelease.

## 3. Verify the GitHub prerelease

Download the archive and checksum from a different directory or machine, then
verify them before sharing the link:

```sh
shasum -a 256 -c markdown-launcher-v0.1.0-macos-$(uname -m).zip.sha256
gh attestation verify markdown-launcher-v0.1.0-macos-$(uname -m).zip \
  --repo arunsaini719/markdown-launcher
ditto -x -k markdown-launcher-v0.1.0-macos-$(uname -m).zip extracted
./scripts/verify-macos-app.sh "extracted/Markdown Launcher.app"
```

Confirm that the release contains `arm64` and `x86_64` archives with matching
checksum files, both provenance attestations verify against this repository,
and the release remains marked as a prerelease. It must still clearly state
that the applications are not Developer ID signed or notarized.

## Correcting a failed release

Fix automation or packaging defects on `main` and create a new patch version.
Do not move a published version tag to different source code. A tag may be
deleted and recreated only when its workflow failed before publishing any
release or downloadable artifact.
