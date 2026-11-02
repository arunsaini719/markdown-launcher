# Security Policy

## Supported versions

Markdown Launcher is still in early development. Security fixes are applied to
the latest code on `main` and included in the next available release.

| Version | Supported |
| --- | --- |
| `0.1.x` | Yes |
| Older versions | No |

## Report a vulnerability

Please do not open a public issue for a suspected vulnerability. Use GitHub's
private vulnerability reporting flow from the repository's **Security** tab and
include:

- The affected version or commit
- The operating system and filesystem involved
- Reproduction steps or a minimal proof of concept
- The expected and observed security boundaries
- Any suggested mitigation

Dependency-changing pull requests are checked for newly introduced known
vulnerabilities at moderate severity or higher. The complete Rust lockfile is
also scanned against RustSec whenever dependencies change, every Monday, and on
demand. GitHub Dependabot alerts and security updates are enabled for newly
reported advisories. Tagged release archives receive signed GitHub
build-provenance attestations so downloaded artifacts can be verified against
this repository. Third-party workflow actions are pinned to immutable commit
SHAs while Dependabot continues tracking their documented release lines. These automated controls supplement rather
than replace responsible disclosure and maintainer review.

Reports involving arbitrary file access, symlink traversal, unsafe replacement
of Markdown files, unintended network access, or exposure of local note content
are especially useful. Remove personal notes, credentials, access tokens, and
other sensitive data from every attachment.

You should receive an acknowledgement within seven days. Please allow time for
a fix and coordinated disclosure before publishing details.
