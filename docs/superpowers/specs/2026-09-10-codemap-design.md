# codemap — Design Spec

**Date:** 2026-09-10
**Status:** Draft, pending review
**Name:** `codemap` is provisional. Registry survey found nearly all short descriptive
names squatted; the binary name is independent of registry names, so the crate and the
npm wrapper (`@<scope>/codemap`) can differ from the command without affecting design.

---

## 1. Problem

While an AI coding agent works through a task, the developer loses the thread. You know
roughly what it is doing because you read the tool calls scroll past, but you do not hold
a picture of *where in the system* the work is landing, what calls the thing being edited,
or which API endpoints are downstream of it. Reconstructing that means reading code — which
is the thing you delegated in the first place.

`codemap` is an ambient, read-mostly display that answers "where is the work happening, and
what does it touch?" continuously, without you asking.

## 2. Goals

- Show, live, which function/class/module the agent is currently reading or editing.
- Make the *shape* of the codebase navigable at any scale without rendering a hairball.
- Answer "which API endpoints can reach this function?" for any node, instantly.
- Work with any agent, with higher fidelity where the agent exposes hooks.
- Stay lightweight enough to leave running all day and forget about.
- Install in one command on a machine that has none of the developer's toolchain.

## 3. Non-goals (v1)

- Runtime tracing. Nodes light up from *authoring* activity, not execution. No profiler,
  no instrumentation, the target app is never run.
- Refactoring, editing, or code modification of any kind. `codemap` reads and displays.
- Cross-repo graphs. One repo per daemon instance.
- Type-accurate call resolution. See §7 for the accuracy contract.
- Team features, sharing, hosting, telemetry.

## 4. Core concept: the universe model

The graph is navigated as nested scales rather than a single flat plane. You stand *inside*
a container and see its children; clicking a child descends into it, `Esc` ascends.

| Level | Standing in | You see | Visual |
|---|---|---|---|
| L0 | Repo | Top-level packages / directories | Galaxies |
| L1 | Package | Modules (files) | Star systems |
| L2 | Module | Classes + module-level functions | Stars and planets |
| L3 | Class | Methods | Moons |

Three mechanics make this navigable rather than merely pretty:

**Portals.** Edges leaving the current container are not dropped. They bundle into markers
on the container's shell, labelled with their destination, and clicking one flies you there.
This is what lets you walk a call chain across the repo without surfacing to L0.

**Glowing ancestor chain.** When the agent touches a node at any depth, every container on
the path from the root down to it lights up. Standing at L0 you can see which galaxy is
burning and descend toward it. Activity is legible at every altitude.

**Container impostors.** A container seen from outside renders as a cheap proxy whose
particle density is proportional to its child count, so a heavy module reads as visibly
denser than a thin one before you enter it.

Within whichever level you are standing on, ordinary focus-and-neighborhood highlighting
applies: the active node is bright, its callers and callees within that level are lit, the
rest are dimmed. The universe model governs navigation; focus governs emphasis.

This model is also what keeps rendering cheap: only one container's children are ever drawn,
which is tens of nodes rather than thousands, regardless of repo size.

## 5. Architecture

```
agent hooks ─────▶ codemap-hook (thin client) ──unix socket──┐
                                                              ▼
filesystem ──notify──────────────────────────────────▶  codemap daemon
                                                              │
                          ┌───────────────────────────────────┤
                          │              │                    │
                    WebSocket        pane state          index (in memory,
                          │              │                no disk cache in v1)
                          ▼              ▼
                  browser client    tmux ticker (optional)
                          │              │
                          └── clipboard / tmux send-keys ──▶ agent
```

Six components, each independently testable:

1. **Adapter layer** (§6) — normalizes agent activity into `ActivityEvent`.
2. **Indexer** (§7) — parses source into the symbol graph, lazily and incrementally.
3. **Resolver** (§8) — maps a file + line range to the innermost enclosing symbol.
4. **Focus engine** (§9) — active node, decaying trail, follow/detach state.
5. **Clients** (§11, §12) — browser 3D and optional tmux ticker.
6. **Back-channel** (§13) — composes prompts and hands them to the agent.

**Lifecycle is explicitly opt-in.** Nothing runs until the developer types `codemap` in a
repo. There is no auto-start, and no background process on machines where it was never
invoked. Hooks are installed globally but their first act is a socket-existence check;
with no daemon running, the hook is a single `stat()` and `exit 0`.

## 6. Adapter layer

The core must not know which agent is upstream. All adapters emit:

```rust
struct ActivityEvent {
    kind:  Read | Write | Search,
    path:  PathBuf,
    range: Option<LineRange>,   // None = file-level only
    agent: AgentId,
    ts:    Instant,
}
```

### 6.1 Tier 1 — hook-based (verified 2026-09-10)

All three targets expose command hooks receiving JSON on stdin with a tool name and the
model's raw tool arguments. Differences are a mapping table, not separate integrations.

| | Claude Code | Codex CLI | Gemini CLI |
|---|---|---|---|
| Pre / post events | `PreToolUse` / `PostToolUse` | `PreToolUse` / `PostToolUse` | `BeforeTool` / `AfterTool` |
| Config | `~/.claude/settings.json` | `~/.codex/hooks.json`, `config.toml` `[hooks]` | `~/.gemini/settings.json`, or bundled in an extension |
| Key payload fields | `tool_name`, `tool_input` | `tool_name`, `tool_input`, `tool_use_id` | `tool_name`, `tool_input`, `tool_response`, `cwd` |
| Matchers | regex | regex | regex (tool events), exact (lifecycle) |
| Install friction | permission prompt | **hook definition must be reviewed and trusted, recorded by hash** | project trust; extensions may bundle hooks |

**Per-agent normalization is the real work.** Codex delivers file edits as a patch inside
`tool_input.command`, so the Codex path needs a patch parser to recover line ranges, where
Claude Code and Gemini supply structured arguments. Each agent gets a `Normalizer` that
turns its `tool_input` into `(kind, path, range)`; everything downstream is shared.

### 6.2 Tier 3 — universal filesystem fallback

`notify` watches the repo, debounced, diffing changed files against last-known content to
derive line ranges. Works with every agent and with hand-editing. Cannot distinguish reads
from writes, and cannot attribute a change to an actor.

**Tier 3 is the compatibility story; Tier 1 is enrichment.** Tier 3 is the first adapter built and is
always active; Tier 1 events, when present, supersede filesystem events for the same file
within a short window to avoid double-counting.

Tier 2 (scraping transcripts or per-turn commits from agents without hooks) is deliberately
deferred. It is brittle and unnecessary now that the three target agents are all Tier 1.

## 7. Indexer and language extractors

**tree-sitter** for all languages: incremental re-parse, native Rust, one dependency shape
across the set.

**Critical memory rule: parse → extract → drop the syntax tree.** Trees are retained only
for files in the currently-focused container. Persistent state is nodes and edges only.
Retaining every tree would cost hundreds of MB and negate the reason for choosing Rust.

**Lazy, level-by-level indexing.** At startup, walk the tree and extract module-level
symbols only — cheap, shallow. Full call-graph extraction for a container happens when the
developer enters it or when the agent touches it. Cold start is near-instant regardless of
repo size; memory stays proportional to where you have actually been.

**Languages (v1):** Python, TypeScript/JavaScript, Go, Rust. Each implements:

```rust
trait LanguageExtractor {
    fn containers(&self, tree: &Tree, src: &str) -> Vec<Node>;   // modules, classes
    fn symbols(&self,    tree: &Tree, src: &str) -> Vec<Node>;   // functions, methods
    fn calls(&self,      tree: &Tree, src: &str) -> Vec<Edge>;
    fn routes(&self,     tree: &Tree, src: &str) -> Vec<Route>;  // API entry points
}
```

**API routes are a first-class node kind**, not a tag: FastAPI/Flask decorators, Express
handlers, Go `http.HandleFunc` and chi/gin routers, Rust axum/actix route registrations.
Labelled with method and path (`POST /chat/stream`), rendered distinctly, placed at the
outer shell of the layout because they are where control enters the system.

**Accuracy contract — stated plainly because it is the tool's main limitation.**
`contains` edges and route detection are exact. `calls` edges are heuristic:

- Explicit imports resolve reliably in all four languages.
- Method dispatch through objects (`self.svc.run()`, `this.svc.run()`) needs type inference
  that tree-sitter does not provide, and will produce missing or wrong edges.
- Go and Rust resolve better than Python and TypeScript, being statically typed with
  explicit imports.
- Dynamic patterns — `getattr`, monkey-patching, duck typing, reflection — are not handled.

Accuracy targets to be validated by the §17 spike, not assumed.

## 8. Resolver

Maps an `ActivityEvent` to the innermost enclosing graph node.

- Read events carry offset/limit directly.
- Write events carry either structured old/new strings (Claude Code, Gemini) or a patch
  (Codex). Structured edits are located by searching the file for the old string; patches
  are parsed for hunk headers.
- Search and shell events resolve to file-level only, or are dropped.
- With a line range in hand, walk the AST to the innermost symbol containing it.

Failure is expected and must be graceful: unresolvable events degrade to the file node,
then to the package node, never to an error.

## 9. Focus engine

Holds the active node and a decaying trail of recently-touched nodes (~90s half-life). The
trail is what makes the display a record of the session rather than a snapshot: glance over
after ten minutes and the still-glowing nodes are where the work has been.

**Follow mode with auto-detach.** The camera follows agent activity by default. The moment
the developer navigates manually it detaches and stays put, showing a
`agent working in <container> →` indicator; one key or a click re-attaches and flies back.
This is the difference between a tool you keep open and one you close after a day.

## 10. Wire protocol

WebSocket, JSON, daemon → client:

- `graph/level` — children and intra-level edges for a container, plus portal bundles
- `graph/patch` — incremental node/edge changes after a re-index
- `focus` — active node, ancestor chain, trail with decay values
- `node/detail` — signature, docstring, source, callers, callees, reachable-from routes

Client → daemon: `navigate`, `request_detail`, `follow_toggle`, `send_to_agent`, `open_editor`.

## 11. Browser client

TypeScript, `3d-force-graph` over three.js. Rust has no business here; WASM would be
ceremony for a thin rendering layer.

Node sizing: kind sets base size, fan-in modulates within kind, so a helper called from
thirty places visibly outweighs one called once.

Color: **active** bright and saturated; **trail** warm and fading; **neighborhood** normal;
**everything else** dimmed or culled.

**Inspector panel** (on click): kind badge, qualified name, `file:line`, signature and
docstring, source body syntax-highlighted and collapsed past ~40 lines, then three clickable
lists — *called by*, *calls*, and **reachable from** (the API routes that can reach this
node). Then two actions: *Send to agent*, *Open in editor*.

Reachability is derived from the existing call graph by upward traversal — no extra
machinery — and is the feature that most directly serves the original goal.

## 12. tmux ticker (optional)

`ratatui`, ~8 lines, one of several sinks rather than an assumed surface. Many users are in
VS Code terminals, iTerm, or Warp, so tmux must never be a hard requirement.

```
◈ core-service ▸ chat ▸ service.py ▸ ChatService          [following]
● stream_response()                              service.py:142
  ←  handle_chat · _retry_wrapper
  →  build_prompt · LLMClient.send · emit_event
  ⇡  POST /chat/stream · POST /chat/batch
  ~  _normalize · build_prompt · handle_chat · LLMClient.send
```

## 13. Back-channel

Clicking a node composes a prompt from a fixed intent menu — *explain*, *write tests*,
*find bugs*, *refactor*, *trace callers* — prefilled with the qualified name and `file:line`.

Delivery degrades gracefully:

1. **tmux `send-keys`** into the agent's pane where tmux is available. Pane discovered from
   `tmux list-panes`, overridable with `--agent-pane %3`.
2. **Clipboard** everywhere else. One paste. Unglamorous and universal.

**The prompt is never auto-submitted.** Text is typed into the pane without pressing Enter.
Injecting a prompt mid-turn while an agent is working would be genuinely destructive. The
tool composes; the developer decides.

**Open in editor** is a configurable command template, defaulting to `$EDITOR`:
`open_command = "nvim --server {sock} --remote +{line} {file}"`.

## 14. Configuration

`.codemap.toml` at the repo root: ignore paths, extra entry points, editor command, adapter
selection, neighborhood depth, trail half-life. Needed the moment the tool is pointed at
someone else's tree.

## 15. Distribution

- **GitHub Releases** with prebuilt binaries for macOS (arm64, x86_64) and Linux, plus a
  `curl | sh` installer. Baseline.
- **npm wrapper** that fetches the right binary, so `npx @<scope>/codemap` works with zero
  install. This matters more than it looks: it is how most JavaScript developers will ever
  try the tool, and follows the esbuild/Biome pattern.
- **Homebrew tap** for macOS.
- `cargo install` as a nice-to-have; it reaches only Rust developers.
- No Docker. It would defeat the lightweight goal and still need host filesystem and tmux.

`codemap install-hooks` writes hook config for whichever of the three agents are detected.
For Codex it must print the exact hook definition for review, and re-print on every change,
because Codex records trust against the definition's hash.

## 16. Privacy

No telemetry. No network calls. Nothing leaves the machine. Source is read from the local
filesystem and rendered to a localhost port bound to loopback only. Stated as an explicit
commitment because it is the first question any developer asks of a tool that reads their
entire codebase.

## 17. Performance budgets

Targets to validate, **not measurements**:

- Hook overhead: < 5ms wall time (socket write and exit; the agent blocks on this).
- Cold start on a ~700-file repo: < 1s to first render (shallow index only).
- Container entry (full extraction): < 200ms.
- Steady-state RSS: < 50MB on a ~700-file repo.
- Frame rate: 60fps at up to ~200 visible nodes.

**First task in the implementation plan is a throwaway spike** measuring tree-sitter over
`core-service`'s 669 Python files: cold index time, node/edge counts, RSS with and without
tree retention, and a manual accuracy sample of `calls` edges. If cold index or accuracy
misses badly, the indexing strategy changes before anything is built on top of it.

## 18. Risks

| Risk | Mitigation |
|---|---|
| `calls` accuracy too low to be useful | Spike measures it first; if poor, fall back to import-level edges plus explicit call sites only, and say so in the UI |
| Nested-scale camera is disorienting | The novel part with no reference implementation; budget a second pass, keep sibling backdrop and breadcrumbs |
| Agent hook APIs shift | Adapters are a mapping table; pin verified schemas in tests and re-verify per release |
| Rapid tool calls thrash the camera | Debounce focus changes; trail absorbs bursts |
| Squatted registry names | Binary name is independent; scoped npm package sidesteps it |

## 19. Open questions

- Should L0 group by directory or by import-coupling clusters? Directory is simpler and
  matches the developer's mental model; coupling is more informative. Start with directory.
- How should multiple concurrent agent sessions on one repo be displayed — merged trail, or
  color-per-agent? Deferred; v1 assumes one session.
