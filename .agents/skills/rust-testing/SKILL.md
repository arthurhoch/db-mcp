---
name: rust-testing
description: Test observable Rust behavior with isolated SQLite files and disposable network databases. Use when adding or changing tests.
---

# Tests

Adapted from affaan-m/ECC. Source revision and license are recorded in
../manifest.json and LICENSE.upstream.

- Cover the changed behavior and failure path. For a bug fix, reproduce the
  failure before changing the implementation when practical.
- Use temporary files and directories. Write tests must never use user connections.
- Use real databases for dialect, binding, transaction and precision checks.
  Mocking a driver cannot prove those behaviors.
- Keep network tests opt-in. Document required containers and native libraries.
- Synchronize on observable state, channels or barriers instead of guessed delays.
- Check persisted data after uncertain commits or failed transactions.
- Keep setup, action and assertions readable. Several assertions are appropriate
  for one contract scenario; do not split solely to meet an assertion count.
- Do not add tests for formatting changes or getters. Do not add coverage tools,
  mocks or fixture frameworks without a need.
- Run cargo test --all-targets --locked and the affected integration tests.
