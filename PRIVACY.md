# DB MCP privacy policy

DB MCP runs as a local process started by your MCP client. It has no telemetry,
analytics, or hosted service.

DB MCP reads connection settings from local TOML files and, when configured,
credentials from environment variables. It uses credentials to authenticate
to the database you select. Keep these files and variables private.

SQL and bound parameters go to the selected database. Query results and
database metadata go back to the MCP client over standard output.
The client and any model provider it uses handle those results under their
own policies. The database and its operators may also retain query activity.

DB MCP does not create a query history. It writes diagnostic logs to standard
error. Logs can contain a request ID, database ID, duration, and outcome.
They exclude connection URLs, credentials, SQL, parameter values, and rows.
Your MCP client or operating system may capture those logs.

For security or privacy issues, see the
[security policy](https://github.com/arthurhoch/db-mcp/security/policy).
