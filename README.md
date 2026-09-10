# codemap

A live map of your codebase that follows what your coding agent is doing.

While an AI agent works, you lose the thread — you can read the tool calls scroll
past, but you don't hold a picture of *where in the system* the work is landing or
what it touches. `codemap` answers that continuously, without you asking.

- **Live focus.** Hooks from Claude Code, Codex CLI and Gemini CLI report every
  read and edit; the graph highlights the exact function being touched and fades a
  trail of what came before.
- **A 3D tree you can fly through.** Project → packages → files → symbols, laid
  out in depth and drawn on canvas. Drag to orbit, wheel to zoom, click to
  select, `/` to jump to any symbol. Level toggles decide how deep it draws.
- **A panel that explains, not just lists.** What a symbol is (its doc comment
  and signature), how you get to it (the call path up to a route or entry
  point), and how much of that codemap actually knows.
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

Run the daemon in its own detached window so it costs you no screen space, then
split a short ticker pane wherever you want it:

```sh
REPO=~/code/my-project
tmux new-window -d -n codemapd "codemap start $REPO"
tmux split-window -v -l 9 "codemap ticker $REPO"
```

Or as a shell function:

```sh
codemap-here() {
  local r="${1:-$PWD}"
  tmux new-window -d -n codemapd "codemap start $r"
  sleep 2
  tmux split-window -v -l 9 "codemap ticker $r"
}
```

`f` toggles follow, `q` quits. The ticker is optional — the browser UI is the
primary surface, since plenty of people don't use tmux.

### Sending a prompt back to your agent

Clicking *send to agent* in the browser types a prompt into your agent's tmux
pane **without pressing Enter** — you review and submit it yourself.

The pane is auto-detected. Note that `pane_current_command` is unreliable
(Claude Code reports its version string, e.g. `2.1.263`, not `claude`), so
detection inspects the processes on each pane's tty. If it picks the wrong pane,
pin it:

```sh
export CODEMAP_AGENT_PANE=%19   # see: tmux list-panes -a -F '#{pane_id} #{pane_current_command}'
```

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

## What gets indexed

Your source, not your dependencies or your test suite. Excluded by default:

- **Vendored / build output** — `node_modules`, `target`, `dist`, `build`, `venv`,
  `site-packages`, `vendor`, `third_party`, `.next`, `coverage`
- **Tests** — `tests/`, `spec/`, `e2e/`, `fixtures/`, and files named `test_*.py`,
  `*_test.go`, `*.test.ts`, `*.spec.ts`, `conftest.py`
- **Generated** — `*_pb2.py`, `*.pb.go`, `*.d.ts`, `*.min.js`, `*.generated.*`,
  `baml_client/`, `__generated__/`
- **Not-the-codebase** — `migrations/`, `alembic/`, `examples/`, `docs/`, `scripts/`

Pass `--all` to include everything. On a real service this is the difference
between 788 modules (46% of them tests) and 360 modules of actual source.

## Languages

Python, TypeScript/JavaScript, Go, Rust. Route detection covers FastAPI/Flask,
Express-style, Go `HandleFunc` and chi/gin, and axum.

## Design

`docs/design/` holds the Claude Design canvas this UI was built from, including
"current vs redesign" artboards for both the browser and the terminal.

## Status

Working: indexer, all four extractors, reachability, the daemon, all three hook
adapters, filesystem fallback, resolver, focus engine, browser UI, ticker,
hook installation.

Not done: packaging (Homebrew tap, npm wrapper, prebuilt binaries), incremental
single-file reindex (a change currently re-indexes the repo, ~800ms for 787 files).
