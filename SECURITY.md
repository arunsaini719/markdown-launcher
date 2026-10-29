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

Reports involving arbitrary file access, symlink traversal, unsafe replacement
of Markdown files, unintended network access, or exposure of local note content
are especially useful. Remove personal notes, credentials, access tokens, and
other sensitive data from every attachment.

You should receive an acknowledgement within seven days. Please allow time for
a fix and coordinated disclosure before publishing details.
