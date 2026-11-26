# GitHub Copilot in VS Code

Extract the [release archive](https://github.com/arthurhoch/db-mcp/releases/latest)
for your system. Add `db-mcp` to `servers` in the workspace's `.vscode/mcp.json`.
If the file is new, use the whole example.

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

Replace both paths with absolute paths from your download. On Windows, use
`db-mcp.exe` and paths such as `C:/tools/db-mcp-windows-x64/db-mcp.exe`.

1. Run **MCP: List Servers** from the Command Palette, select `db-mcp` and
   choose **Start**. Accept the trust prompt if VS Code shows one.
2. In a Copilot Chat agent session, enable `list_databases`, `describe_database`
   and `execute_sql`.
3. Ask “Which order IDs are pending in demo-ro?” The demo returns **101** and **103**.

To use the same entry across workspaces, open **MCP: Open User Configuration**.
It uses the same [configuration format](https://code.visualstudio.com/docs/agents/reference/mcp-configuration).

## Use your databases

Create and validate your [database configuration](configuration.md). Point
`--config` at the global TOML and restart `db-mcp` from **MCP: List Servers**.
The server inherits its environment from VS Code. If startup fails, select
`db-mcp` in that menu and choose **Show Output**.
