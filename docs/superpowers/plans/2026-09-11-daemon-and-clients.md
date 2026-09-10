# codemap Daemon and Clients — Implementation Record

> **STATUS: complete.** Built in one pass on 2026-09-11 rather than written as a
> plan first, at the user's explicit request ("finish the entire project in one go").
> This document records what was built and what was deliberately left out.

**Spec:** `docs/superpowers/specs/2026-09-10-codemap-design.md`

## Delivered

| Spec section | Component | Where |
|---|---|---|
| 6 | `ActivityEvent` contract | `codemap-daemon/src/event.rs` |
| 6.1 | Claude Code / Codex / Gemini normalizers | `codemap-daemon/src/adapter.rs` |
| 6.2 | Tier 3 filesystem watcher, hook suppression | `codemap-daemon/src/watch.rs` |
| 8 | Resolver, file+range to innermost symbol | `codemap-daemon/src/resolver.rs` |
| 9 | Focus engine, 90s half-life trail, follow/detach | `codemap-daemon/src/focus.rs` |
| 10 | WebSocket protocol | `codemap-daemon/src/state.rs` |
| 5, 13 | Daemon, unix socket, back-channel | `codemap-daemon/src/server.rs` |
| 4, 11 | Browser client | `codemap-daemon/client/index.html` |
| 12 | ratatui ticker | `codemap-cli/src/ticker.rs` |
| 15 | `install-hooks` for all three agents | `codemap-cli/src/hooks.rs` |

## Verified, not assumed

- Full loop exercised end to end: a simulated Claude Code `Edit` hook resolved to
  `server.connect_idempotency_credential_id` (lines 290-304) in `core-service`,
  with its ancestor chain, and reached both the browser and the tmux ticker.
- Browser client rendered in headless Chromium: HTTP 200, zero console errors,
  zero uncaught exceptions, graph visibly drawn with 14,652 nodes.
- Ticker rendered in a real tmux pane.
- Hook latency measured over 50 runs each way (see spec section 17).

## Deliberately not built

- **Packaging.** No Homebrew tap, no npm wrapper, no prebuilt release binaries.
  `cargo build --release` is currently the only install path.
- **Incremental reindex.** A file change triggers a full repo reindex (~805ms for
  787 files). Correct but wasteful; the most obvious remaining optimization.
- **`.codemap.toml`.** Configuration is environment variables today
  (`CODEMAP_AGENT_PANE`, `CODEMAP_OPEN_COMMAND`, `EDITOR`).
- **Multi-agent sessions.** One focus state per repo; concurrent agents merge into
  one trail rather than getting a color each (spec section 19).
- **`codemap stop` is soft.** It removes the socket; the process exits on its next
  request rather than immediately.
