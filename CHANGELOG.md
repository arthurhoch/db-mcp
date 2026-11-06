# Changelog

## 0.2.1

- Shorten the `execute_sql` tool description and keep database context instructions in the MCP server instructions.

## 0.2.0

- Read Microsoft SQL Server over verified TLS with a SQL login.
- Inspect indexes and constraints with `describe_database`.
- Get estimated query plans with `explain_query` on all five databases.
- Test a configured connection with `--test-connection <database_id>`.
- Include the privacy policy in release packages.

## 0.1.2

- Support Oracle accounts with only an `11G` password verifier, including on Oracle 19c.

## 0.1.1

- Update the direct `base64` dependency from 0.22.1 to 0.23.1.

## 0.1.0

- Oracle, PostgreSQL, MySQL and SQLite over MCP STDIO.
- TOML connection catalog with descriptions, credentials and three access modes.
- Native SQL bindings, bounded results and ordered transactional batches.
- Oracle package calls and PostgreSQL transaction-local context for VPD/RLS.
- Binaries for macOS, Linux and Windows on x64 and ARM64, with a SQLite demo.
