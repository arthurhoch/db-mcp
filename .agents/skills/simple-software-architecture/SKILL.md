---
name: simple-software-architecture
description: Keep Rust modules and responsibility boundaries direct. Use when changing architecture, validation ownership or technical documentation.
---

# Architecture

Adapted from iam-apps/.agents/skills/simple-software-architecture for Rust.

- Keep a rule with its owner: configuration in config, SQL authorization in
  policy, call coordination in execution, protocol details in mcp and native
  behavior in database adapters.
- Prefer direct calls and cohesive modules. Do not add a layer, trait or wrapper
  unless it removes a current duplication or represents a real boundary.
- Keep transaction ownership visible: where it begins, what executes and how
  commit, rollback and uncertainty are reported.
- Use structs, enums and Result. Related contract types may share a module;
  do not require one file per type or class-style application layers.
- Async is required for the server. Keep native blocking work outside Tokio's
  async executor and bound its concurrency.
- Preserve the user's scope. Do not add HTTP, authentication, an internal database
  or infrastructure services as preparation for possible requirements.
- Document each behavior once. Link to its owner instead of maintaining repeated
  requirements, checklists and status reports.
