---
name: rust-async-patterns
description: Handle Tokio concurrency, cancellation and blocking drivers in DB MCP. Use when changing connection admission, timeouts or async execution.
---

# Async execution

Adapted from wshobson/agents. Source revision and license are recorded in
../manifest.json and LICENSE.upstream.

- Bound operations and connections with semaphores; do not spawn unlimited tasks.
- Acquire the per-database permit before global permits so one destination
  cannot reserve all global slots while waiting.
- Keep synchronous Oracle I/O in spawn_blocking. Move permits into that closure;
  cancelling the future does not stop blocking I/O or release its resources.
- Use CancellationToken and select! at operation boundaries. Treat a write
  interrupted after send as unknown unless commit or rollback was confirmed.
- Do not hold synchronous locks across await. Keep shared mutable state limited
  to the progress signals needed by cancellation.
- Close each operation's connection. Never reuse a session with uncertain state.
- Do not add automatic SQL retries; a missing response does not prove failure.
