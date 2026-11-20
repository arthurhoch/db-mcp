# Security

Report vulnerabilities privately through
[GitHub security advisories](https://github.com/arthurhoch/db-mcp/security/advisories/new).
Include the affected version, reproduction steps and expected impact. Use
disposable data and remove connection credentials from examples and logs.

The latest release receives security fixes. Dependabot, RustSec and CodeQL
monitor dependencies, Rust source and GitHub Actions workflows.

The audit configuration accepts RUSTSEC-2026-0543, a maintenance notice for
`chrono`. Oracle and the MCP SDK still require it, and the advisory has no
patched version. Review this exception when those dependencies provide a
maintained replacement.
