# DB MCP

<!-- mcp-name: io.github.arthurhoch/db-mcp -->

[![M8ven Score](https://m8ven.ai/badge/mcp/arthurhoch-db-mcp-10phce?v=01c9b4c4da3cb37b05ed9721bf76fa14)](https://m8ven.ai/mcp/arthurhoch-db-mcp-10phce?s=readme)
[![DB MCP on Glama](https://glama.ai/mcp/servers/arthurhoch/db-mcp/badges/score.svg)](https://glama.ai/mcp/servers/arthurhoch/db-mcp)
[![DB MCP on mcprush](https://mcprush.com/badge/arthurhoch-db-mcp/score.svg)](https://mcprush.com/to/arthurhoch-db-mcp?ref=badge)
[![MCP Registry](https://img.shields.io/badge/MCP_Registry-db--mcp-blue)](https://registry.modelcontextprotocol.io/?q=io.github.arthurhoch%2Fdb-mcp)
[![Release](https://img.shields.io/github/v/release/arthurhoch/db-mcp)](https://github.com/arthurhoch/db-mcp/releases/latest)
[![CI](https://github.com/arthurhoch/db-mcp/actions/workflows/ci.yaml/badge.svg?branch=main&event=push)](https://github.com/arthurhoch/db-mcp/actions/workflows/ci.yaml?query=branch%3Amain)
[![License](https://img.shields.io/github/license/arthurhoch/db-mcp)](LICENSE)

DB MCP lets your MCP client run SQL against databases you configure locally.
It supports reads and writes on Oracle, PostgreSQL, MySQL and SQLite. SQL Server
supports reads only. Each connection has an ID and an access mode in its own TOML file.

## Install with an agent

Paste this into a coding agent with terminal access.

```text
Install DB MCP for my MCP client. Ask which client I use if you can't tell.
Download the archive for my OS and CPU from https://github.com/arthurhoch/db-mcp/releases/latest,
check it against SHA256SUMS from the same release, and extract it to a permanent location.
For a fresh install, register a stdio server with absolute paths to db-mcp and
the bundled demo/config/global.toml. Pass the TOML path with --config. Keep
existing client entries and catalogs. Run db-mcp --config <global.toml> --check-config.
With the demo loaded, ask the client for the pending order IDs in demo-ro.
Expect 101 and 103. If the client needs a restart first, give me the query to run.
```

## Add a database with an agent

```text
Add one connection to my DB MCP catalog. Ask which engine and database to use,
what to call it, and what data it contains. Use read_only unless I request write
or DDL access. SQL Server always uses read_only. For SQLite, ask for the existing
file path. For a network database, ask for the host, port, database or service name,
and username. Set password_env to the name of a variable I'll supply locally.
Keep its value out of the chat, commands and TOML.

Use the matching TOML in config-examples/databases from the release as a template.
If I'm using the bundled demo, create a personal catalog outside the install directory.
Give the connection a unique ID and add its file under the catalog's databases_dir.
Run db-mcp --config <global.toml> --check-config, then repeat with --test-connection <id>.
Once both pass, point my client at that catalog and restart the MCP server.
```

The [configuration guide](https://github.com/arthurhoch/db-mcp/blob/main/docs/configuration.md) has a TOML example for each database engine.

## Download and run

The archives include the executable, a SQLite demo and sample connection files.
The executable runs without a Rust toolchain or Oracle Instant Client.

| System | x64 (Intel/AMD) | ARM64 (Apple Silicon/ARM) |
| --- | --- | --- |
| macOS | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-macos-x64.tar.gz) | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-macos-arm64.tar.gz) |
| Linux | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-linux-x64.tar.gz) | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-linux-arm64.tar.gz) |
| Windows | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-windows-x64.zip) | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-windows-arm64.zip) |

On a Mac with Apple Silicon, try the demo from a terminal.

```sh
curl -fLO https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-macos-arm64.tar.gz
tar -xzf db-mcp-macos-arm64.tar.gz
cd db-mcp-macos-arm64
./db-mcp --version
./db-mcp --config demo/config/global.toml --check-config
./db-mcp --config demo/config/global.toml --test-connection demo-ro
```

On Windows, extract the ZIP and use `db-mcp.exe`. If your client supports MCPB
and you already have a global TOML, install the matching `.mcpb` from the
[release](https://github.com/arthurhoch/db-mcp/releases/latest) and select that TOML when prompted.

For checksums and build provenance, see the [verification instructions](https://github.com/arthurhoch/db-mcp/blob/main/docs/releases.md#verify-a-download).

## Connect your MCP client

### GitHub Copilot in VS Code

The [Copilot guide](https://github.com/arthurhoch/db-mcp/blob/main/docs/copilot.md) shows the `.vscode/mcp.json` entry and how to start the server.

### Codex

Add this entry to your Codex configuration. Replace both paths with absolute paths on your machine.

```toml
[mcp_servers.db_mcp]
command = "/absolute/path/db-mcp-macos-arm64/db-mcp"
args = ["--config", "/absolute/path/db-mcp-macos-arm64/demo/config/global.toml"]
```

## Access modes

| Connection mode | Operations |
| --- | --- |
| `read_only` | Read queries. |
| `read_write` | Read queries, INSERT, UPDATE and DELETE. |
| `read_write_ddl` | Read and write, plus supported CREATE, ALTER and DROP statements. |

The database user's grants also apply. For PostgreSQL RLS or Oracle VPD, send the
context setup and dependent queries in the [same batch](https://github.com/arthurhoch/db-mcp/blob/main/docs/mcp.md#contexto-e-consultas).
If a write returns `effect=unknown`, check the database before retrying it.

## Build from source

From the repository root, use rustup with the pinned toolchain.

```sh
cargo build --release --locked
cargo run --locked --example create_demo
target/release/db-mcp --config .runtime/demo/config/global.toml --test-connection demo-ro
```

The example creates `.runtime/demo` on its first run. To create another copy,
pass a new directory after `--example create_demo --`.

[Documentation](https://github.com/arthurhoch/db-mcp/tree/main/docs) · [Privacy policy](PRIVACY.md)
