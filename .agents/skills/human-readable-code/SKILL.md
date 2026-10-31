---
name: human-readable-code
description: Write and review readable Rust with clear names, explicit ownership, local state and rustfmt. Use for source, tests and documentation cleanup.
---

# Human-readable Rust

Adapted from iam-apps/.agents/skills/human-readable-code.

- Keep related operations and types in cohesive modules.
- Name values by their role. Add type annotations where inference hides a conversion.
- Use early returns for invalid input and match for branches with distinct effects.
- Prefer borrowing when no ownership transfer is needed.
- Use blank lines between phases; format macro bodies and JSON when rustfmt leaves them dense.
- Extract a function when its name clarifies a step or removes repeated mechanics.
- Comments explain constraints or decisions; remove narration of obvious code.
- Documentation states behavior, commands and limits. Remove repeated claims,
  conversation history and unimplemented examples.

For Rust code, read [the Rust reference](references/rust.md).
In formatting-only work, preserve behavior and avoid unrelated edits.
