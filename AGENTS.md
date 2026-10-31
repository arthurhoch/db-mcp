# DB MCP

Prefix shell commands with `rtk`; use `rtk proxy` when no wrapper exists.

Read the relevant contract in docs before changing behavior. Keep the code,
examples and documentation consistent. Application code, examples and tests
are Rust. Do not add scripts in other languages.

Use the project skills in .agents/skills: human-readable-code for readability,
rust-best-practices for Rust, simple-software-architecture for module boundaries,
rust-async-patterns for concurrency and rust-testing for tests.

Keep database ID and access mode explicit per call. Never log credentials,
connection URLs, raw SQL, parameters or rows. Server stdout belongs to MCP;
logs go to stderr. Do not retry SQL after sending it. Preserve uncertain effects.

Run cargo fmt --check, cargo clippy --all-targets --locked -- -D warnings and
cargo test --all-targets --locked. Network tests require disposable databases;
never use user configurations for write tests. Record which checks ran.
