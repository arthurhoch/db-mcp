---
name: rust-best-practices
description: Apply Rust ownership, typed errors and module conventions when writing or reviewing this project.
license: MIT
---

# Rust

Adapted from Apollo GraphQL's rust-best-practices skill. Source revision and
license are recorded in ../manifest.json and LICENSE.upstream.

- Borrow inputs when the callee only reads them; take ownership when needed.
- Use Result for fallible operations and Option for absence. Avoid operational
  panic, unwrap and expect in production code.
- Map driver errors at the adapter boundary. Keep stable codes without exposing
  raw errors that may contain SQL or credentials.
- Use enums for closed alternatives. Add newtypes when they enforce an invariant
  or prevent identity mixups.
- Prefer direct concrete implementations. Add traits or generics for an actual
  shared contract; do not create forwarding layers for symmetry.
- Keep mutation local. Avoid redundant clones and temporary collections, but
  preserve clarity when choosing between loops and iterator chains.
- Add dependencies only for a concrete need. Keep Cargo.lock and the pinned toolchain.
- Use rustfmt and run Clippy for all targets with warnings denied.
