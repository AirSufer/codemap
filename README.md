# codemap

A live map of your codebase that follows what your coding agent is doing.

While an AI agent works, you lose the thread — you can read the tool calls scroll
past, but you don't hold a picture of *where in the system* the work is landing or
what it touches. `codemap` answers that continuously, without you asking.

- **Live focus.** Hooks from Claude Code, Codex CLI and Gemini CLI report every
  read and edit; the graph highlights the exact function being touched and fades a
  trail of what came before.
- **Universe navigation.** Packages contain modules contain classes contain
  functions. Click to descend, `Esc` to ascend. Only one container renders at a
  time, so a 12,000-function repo never becomes a hairball.
- **Reachability.** Select any function and see which API routes can reach it.
- **Works with any agent.** A filesystem watcher covers everything, including you
  editing by hand. Hooks are enrichment, not a requirement.

## Install

```sh
cargo build --release        # single binary, no runtime, no node_modules
./target/release/codemap --help
```

## Use

```sh
codemap start .              # index, serve the UI, open a socket for hooks
codemap install-hooks        # write hook config for installed agents
codemap ticker .             # optional text pane, for tmux beside your agent
codemap status .
codemap stop .
```

Then open the printed `http://127.0.0.1:7878`.

Without starting the daemon, two offline commands work anywhere:

```sh
codemap index <path>              # summary + call-resolution coverage
codemap reach <path> <symbol>     # which routes reach this symbol
```

### tmux

```sh
tmux split-window -v -l 10 'codemap ticker /path/to/repo'
```

`f` toggles follow, `q` quits. The ticker is optional — the browser UI is the
primary surface, since plenty of people don't use tmux.

## What it is honest about

Call edges are **best-effort**, not complete. `codemap` reads syntax, not types,
so a call through a variable whose type it can't infer produces no edge. Measured
internal call resolution:

| Language | Resolution |
|---|---|
| Python | 44.5% |
| TypeScript / JavaScript | 18.5% |
| Rust | 12.8% |

Every node carries an `unresolved_calls` count, and "no route found" is never
rendered as "not reachable". Containment structure and route detection are exact;
only `calls` edges are heuristic.

## Privacy

No telemetry, no network calls, nothing leaves your machine. The UI binds to
loopback and is embedded in the binary — no CDN, no external assets.

## Languages

Python, TypeScript/JavaScript, Go, Rust. Route detection covers FastAPI/Flask,
Express-style, Go `HandleFunc` and chi/gin, and axum.

## Status

Working: indexer, all four extractors, reachability, the daemon, all three hook
adapters, filesystem fallback, resolver, focus engine, browser UI, ticker,
hook installation.

Not done: packaging (Homebrew tap, npm wrapper, prebuilt binaries), incremental
single-file reindex (a change currently re-indexes the repo, ~800ms for 787 files).
