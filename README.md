# DB MCP

An MCP server in Rust for querying and modifying data in Oracle, PostgreSQL,
MySQL and SQLite. Each connection has a TOML file with its description,
credentials and permissions. The agent selects a database by ID and executes
SQL in its native dialect.

## Download and run

Download a package from [Releases](https://github.com/arthurhoch/db-mcp/releases/latest).
Rust and Oracle Instant Client are not required. Packages include the executable,
a SQLite demo and configuration examples.

| System | x64 (Intel/AMD) | ARM64 (Apple Silicon/ARM) |
| --- | --- | --- |
| macOS | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-macos-x64.tar.gz) | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-macos-arm64.tar.gz) |
| Linux | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-linux-x64.tar.gz) | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-linux-arm64.tar.gz) |
| Windows | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-windows-x64.zip) | [Download](https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-windows-arm64.zip) |

For example, on a Mac with Apple Silicon:

```sh
curl -fLO https://github.com/arthurhoch/db-mcp/releases/latest/download/db-mcp-macos-arm64.tar.gz
tar -xzf db-mcp-macos-arm64.tar.gz
cd db-mcp-macos-arm64
./db-mcp --version
./db-mcp --config demo/config/global.toml --check-config
```

On Windows, extract the ZIP and use `db-mcp.exe`. Configuration validation opens
no database. To query the demo, connect your MCP client to this executable with
`--config` pointing to the demo's global TOML, then ask it to list pending orders
in `demo-ro`.

Each release includes `SHA256SUMS` and GitHub build provenance. See
[releases and verification](docs/releases.md) for verification and publishing.

## Build and test

Install Rust through rustup. The toolchain version is pinned in rust-toolchain.toml.

```sh
cargo build --release --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

The executable is built at `target/release/db-mcp` or `target\release\db-mcp.exe`.
Oracle uses [oracledb](https://github.com/oracle/rust-oracledb), a Rust driver
that connects directly without installing Instant Client or Oracle libraries.

## Try it with SQLite

```sh
cargo run --locked --example create_demo
target/release/db-mcp --config .runtime/demo/config/global.toml --check-config
```

Downloaded packages already include the demo. The Rust example creates sample
orders and three connections: `demo-ro`, `demo-rw`
and `demo-ddl`. It refuses to overwrite an existing directory. To use a different
destination, pass its path after `--example create_demo --`.

## Connect to an agent

Example for a client using the Codex TOML configuration format:

```toml
[mcp_servers.db_mcp]
command = "/absolute/path/db-mcp-macos-arm64/db-mcp"
args = ["--config", "/absolute/path/db-mcp-macos-arm64/demo/config/global.toml"]
```

Adjust the executable and configuration paths for your download or source build.
The transport is STDIO. The process uses the catalog from the global configuration
file, with no separate MCP login. Credential environment variables must be available
to the process. Other MCP clients have their own configuration formats.

Available tools: `list_databases`, `describe_database` and `execute_sql`.
Example request: “In demo-ro, list the pending orders.”

| Connection mode | Operations |
| --- | --- |
| `read_only` | Read queries. |
| `read_write` | Read queries, INSERT, UPDATE and DELETE. |
| `read_write_ddl` | Also supported CREATE, ALTER and DROP statements. |

Batches use one transaction per call. DDL statements run individually. The server
reports truncated results and unknown effects; it does not retry SQL automatically.

For RLS/VPD, put context setup and dependent queries in the same `statements` batch.
Oracle supports packaged `CALL`; PostgreSQL supports `SET LOCAL` and custom
`set_config(..., true)`. Statements run in order on the same connection. Oracle
packages that commit internally prevent a guaranteed rollback of the whole batch.

[Configuration](docs/configuration.md) · [MCP contract](docs/mcp.md) ·
[Databases](docs/database-adapters.md) · [Architecture](docs/architecture.md) ·
[Testing](docs/testing.md) · [Skills](docs/skills.md)
