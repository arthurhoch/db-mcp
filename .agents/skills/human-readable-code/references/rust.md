# Rust readability

Use snake_case for modules/functions/fields, PascalCase for types and variants,
and SCREAMING_SNAKE_CASE for constants. Rustfmt controls layout.

Borrow &str, &[T] or &T for read-only inputs. Accept owned values when retaining
or consuming them. Keep clones where ownership requires them; do not clone to
hide unclear lifetimes. Use enums for closed alternatives and Result for failures.

Keep validation, connection acquisition, execution and commit/rollback visible
in the main flow. Iterator chains fit simple transformations; loops usually make
mutable state and early exits easier to follow. Related structs may share a module.

Expand json!, tokio::select! and macro bodies when their shape matters. Avoid
decorative alignment and arbitrary limits on function length or file count.
Do not reformat unrelated code during a scoped change.

In DB MCP, keep database ID, mode, transaction and cancellation state explicit.
Do not obscure driver behavior behind forwarding wrappers. Preserve typed errors
and the distinction between confirmed and unknown effects. Never expose secrets
through Debug, error context or logs.

Use bounded spawn_blocking for synchronous Oracle calls. Its owned permits must
last until I/O finishes, including after the caller stops waiting.
Bind SQL values separately from SQL text.
