# Releases

Pushes and pull requests run formatting, Clippy, tests and release builds on
macOS, Linux and Windows, for x64 and ARM64. A separate Linux ARM64 job runs all
four database suites against disposable containers. CI artifacts are available
for 14 days from the workflow run.

Security checks include RustSec dependency auditing, CodeQL for Rust and Actions,
workflow linting and dependency review on pull requests. Dependabot checks Cargo
and Actions weekly; the security workflow also runs daily. Actions are pinned to
commit hashes, and release publishing has its own write permissions.

## Publish a version

Update `Cargo.toml`, `Cargo.lock` and `CHANGELOG.md` in a pull request. Merge after
CI passes, then tag that commit:

```sh
git switch main
git pull --ff-only
git tag -a v0.1.1 -m "DB MCP 0.1.1"
git push origin v0.1.1
```

Use `vX.Y.Z` matching the Cargo package version. The release workflow verifies
the tag, rebuilds and tests all six packages, runs security checks, generates
SHA-256 checksums and attests provenance before publishing all assets together.
Packages include a ready-to-use SQLite demo and configuration examples. Existing
release assets are not overwritten.

The manual Release workflow accepts an existing tag; choose that same tag as the
workflow's Git ref. The workflow rejects a different triggering commit.

## Verify a download

Download the package and `SHA256SUMS` from the same release. For macOS ARM64:

```sh
grep ' db-mcp-macos-arm64.tar.gz$' SHA256SUMS | shasum -a 256 -c -
gh attestation verify db-mcp-macos-arm64.tar.gz --repo arthurhoch/db-mcp
```

On Linux, use `sha256sum -c` in place of `shasum -a 256 -c`. On Windows, compare
`Get-FileHash .\db-mcp-windows-x64.zip -Algorithm SHA256` with the corresponding
entry in `SHA256SUMS`. These commands do not require Rust.

Windows packages link the C runtime statically. The Linux binaries target glibc
systems using the Ubuntu 24.04 build environment.
macOS packages are not notarized with an Apple Developer identity; GitHub
provenance identifies the workflow that produced each archive.
