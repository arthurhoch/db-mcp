# GitHub Copilot in VS Code

DB MCP runs as a local STDIO server. VS Code starts the executable and manages
the process while the server is in use. No separate daemon or open terminal is
required. See [VS Code server management](https://code.visualstudio.com/docs/agents/run/manage-mcp-servers).

## Configure the server

1. Download and extract the package for your operating system and architecture
   from [Releases](https://github.com/arthurhoch/db-mcp/releases/latest).
2. Create `.vscode/mcp.json` in the workspace where you use Copilot:

```json
{
  "servers": {
    "db-mcp": {
      "type": "stdio",
      "command": "/absolute/path/db-mcp-macos-arm64/db-mcp",
      "args": [
        "--config",
        "/absolute/path/db-mcp-macos-arm64/demo/config/global.toml"
      ]
    }
  }
}
```

Replace both paths with the extracted executable and demo configuration. On
Linux, use the corresponding Linux package directory. On Windows, use
`db-mcp.exe` and JSON paths such as `C:/tools/db-mcp-windows-x64/db-mcp.exe`.
Spaces in paths are supported; keep each argument as its own JSON string.

3. Open the Command Palette, run **MCP: List Servers**, select `db-mcp` and
   choose **Start**. Accept the server trust prompt if VS Code shows one.
4. In Copilot Chat, use an agent session and enable the server's tools:
   `list_databases`, `describe_database` and `execute_sql`.
5. Ask: **"In demo-ro, list the pending orders."**

The demo contains pending orders 101 and 103. You can use the server with any
workspace; the paths above point to the same downloaded executable and catalog.
For a user-profile configuration shared across workspaces, use **MCP: Open User
Configuration**. Both locations use the `servers` object shown above; see the
[VS Code MCP configuration reference](https://code.visualstudio.com/docs/agents/reference/mcp-configuration).

## Use your databases

Create a global TOML and one TOML per connection, following
[database configuration](configuration.md). Change the path after `--config`
to your global TOML. Database IDs, credentials and access modes belong in these
files. Environment variables referenced by `url_env`, `username_env` or
`password_env` must be available to the server process started by VS Code.

For Oracle or PostgreSQL context setup, ask Copilot to send the setup and queries
in the same `execute_sql` batch. See [context and queries](mcp.md#contexto-e-consultas).

## Check startup

Validate the catalog before starting it in VS Code:

```sh
/absolute/path/db-mcp-macos-arm64/db-mcp --config /absolute/path/db-mcp-macos-arm64/demo/config/global.toml --check-config
```

This checks configuration without opening database connections. To inspect a
startup error, use **MCP: List Servers → db-mcp → Show Output**. Restart the
server from the same menu after changing its TOML files.
