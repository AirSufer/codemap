# codemap Core Index — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A CLI that indexes a repo into a symbol graph and answers "which API routes can reach this function?"

**Architecture:** Three crates. `codemap-graph` holds the data model and graph queries with zero parsing dependencies, so it tests in milliseconds. `codemap-index` owns tree-sitter and one extractor per language behind a common trait. `codemap-cli` wires them together. Indexing is eager and whole-repo (measured 805ms on 787 files), and syntax trees are dropped immediately after extraction (measured 16.7x memory difference).

**Tech Stack:** Rust 1.96, tree-sitter 0.27, tree-sitter-{python 0.25, typescript 0.23, go 0.25, rust 0.24}, ignore 0.4, serde/serde_json 1, clap 4.6, insta 1.48.

**Spec:** `docs/superpowers/specs/2026-09-10-codemap-design.md`

## Global Constraints

- Rust edition 2021, toolchain 1.96.0.
- Exact dependency versions: `tree-sitter = "0.27"`, `tree-sitter-python = "0.25"`, `tree-sitter-typescript = "0.23"`, `tree-sitter-go = "0.25"`, `tree-sitter-rust = "0.24"`, `ignore = "0.4"`, `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`, `clap = { version = "4.6", features = ["derive"] }`, `insta = "1.48"`.
- **Never retain a `tree_sitter::Tree` beyond the function that created it.** Extract, then drop. This is the single rule that keeps RSS at 16MB instead of 266MB.
- `calls` edges are best-effort and must never be presented as complete. Every graph carries an `unresolved_calls` count per node.
- No network calls anywhere in this crate tree. No telemetry.
- `cargo clippy -- -D warnings` and `cargo fmt --check` must pass before every commit.

---

### Task 1: Workspace and graph data model

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/codemap-graph/Cargo.toml`
- Create: `crates/codemap-graph/src/lib.rs`
- Create: `crates/codemap-graph/src/node.rs`
- Create: `crates/codemap-graph/src/graph.rs`
- Test: inline `#[cfg(test)]` in `graph.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `NodeId(u32)`, `NodeKind::{Package, Module, Class, Function, Route}`, `EdgeKind::{Contains, Calls, Imports}`, `Node { id, kind, name, qualified_name, file, line_start, line_end, unresolved_calls }`, `Graph::{new, add_node, add_edge, node, children, parents_of, callers, callees, len}`.

- [ ] **Step 1: Write the failing test**

In `crates/codemap-graph/src/graph.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn f(g: &mut Graph, name: &str) -> NodeId {
        g.add_node(Node::new(NodeKind::Function, name, "m.py", 1, 2))
    }

    #[test]
    fn containment_and_calls_are_separate_relations() {
        let mut g = Graph::new();
        let m = g.add_node(Node::new(NodeKind::Module, "m", "m.py", 1, 99));
        let a = f(&mut g, "a");
        let b = f(&mut g, "b");
        g.add_edge(m, a, EdgeKind::Contains);
        g.add_edge(m, b, EdgeKind::Contains);
        g.add_edge(a, b, EdgeKind::Calls);

        assert_eq!(g.children(m), vec![a, b]);
        assert_eq!(g.callees(a), vec![b]);
        assert_eq!(g.callers(b), vec![a]);
        assert!(g.callees(b).is_empty());
        assert_eq!(g.len(), 3);
    }

    #[test]
    fn qualified_name_defaults_to_name_until_set() {
        let n = Node::new(NodeKind::Function, "run", "m.py", 1, 2);
        assert_eq!(n.qualified_name, "run");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codemap-graph`
Expected: FAIL — `Graph`, `Node`, `NodeKind`, `EdgeKind` do not exist.

- [ ] **Step 3: Write minimal implementation**

`crates/codemap-graph/src/node.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind { Package, Module, Class, Function, Route }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind { Contains, Calls, Imports }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,
    pub qualified_name: String,
    pub file: String,
    pub line_start: u32,
    pub line_end: u32,
    /// Call sites inside this node that could not be resolved to a target.
    /// Surfaced in the UI so the graph never implies complete coverage.
    pub unresolved_calls: u32,
}

impl Node {
    pub fn new(kind: NodeKind, name: &str, file: &str, line_start: u32, line_end: u32) -> Self {
        Node {
            id: NodeId(0),
            kind,
            name: name.to_string(),
            qualified_name: name.to_string(),
            file: file.to_string(),
            line_start,
            line_end,
            unresolved_calls: 0,
        }
    }
}
```

`crates/codemap-graph/src/graph.rs` (above the test module):

```rust
use crate::node::{EdgeKind, Node, NodeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Edge { pub from: NodeId, pub to: NodeId, pub kind: EdgeKind }

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Graph { nodes: Vec<Node>, edges: Vec<Edge> }

impl Graph {
    pub fn new() -> Self { Self::default() }

    pub fn add_node(&mut self, mut n: Node) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        n.id = id;
        self.nodes.push(n);
        id
    }

    pub fn add_edge(&mut self, from: NodeId, to: NodeId, kind: EdgeKind) {
        self.edges.push(Edge { from, to, kind });
    }

    pub fn node(&self, id: NodeId) -> &Node { &self.nodes[id.0 as usize] }
    pub fn nodes(&self) -> &[Node] { &self.nodes }
    pub fn edges(&self) -> &[Edge] { &self.edges }
    pub fn len(&self) -> usize { self.nodes.len() }
    pub fn is_empty(&self) -> bool { self.nodes.is_empty() }

    fn out(&self, id: NodeId, kind: EdgeKind) -> Vec<NodeId> {
        self.edges.iter()
            .filter(|e| e.from == id && e.kind == kind)
            .map(|e| e.to).collect()
    }
    fn inc(&self, id: NodeId, kind: EdgeKind) -> Vec<NodeId> {
        self.edges.iter()
            .filter(|e| e.to == id && e.kind == kind)
            .map(|e| e.from).collect()
    }

    pub fn children(&self, id: NodeId) -> Vec<NodeId> { self.out(id, EdgeKind::Contains) }
    pub fn parents_of(&self, id: NodeId) -> Vec<NodeId> { self.inc(id, EdgeKind::Contains) }
    pub fn callees(&self, id: NodeId) -> Vec<NodeId> { self.out(id, EdgeKind::Calls) }
    pub fn callers(&self, id: NodeId) -> Vec<NodeId> { self.inc(id, EdgeKind::Calls) }
}
```

`crates/codemap-graph/src/lib.rs`:

```rust
pub mod graph;
pub mod node;
pub use graph::{Edge, Graph};
pub use node::{EdgeKind, Node, NodeId, NodeKind};
```

Workspace `Cargo.toml`:

```toml
[workspace]
members = ["crates/codemap-graph", "crates/codemap-index", "crates/codemap-cli"]
resolver = "2"

[workspace.package]
edition = "2021"
rust-version = "1.96"

[workspace.dependencies]
tree-sitter = "0.27"
tree-sitter-python = "0.25"
tree-sitter-typescript = "0.23"
tree-sitter-go = "0.25"
tree-sitter-rust = "0.24"
ignore = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4.6", features = ["derive"] }
insta = "1.48"
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codemap-graph`
Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy -p codemap-graph -- -D warnings
git add Cargo.toml crates/codemap-graph
git commit -m "feat(graph): add node/edge model and adjacency queries"
```

---

### Task 2: Reachability query

**Files:**
- Create: `crates/codemap-graph/src/reach.rs`
- Modify: `crates/codemap-graph/src/lib.rs`
- Test: inline `#[cfg(test)]` in `reach.rs`

**Interfaces:**
- Consumes: `Graph`, `NodeId`, `NodeKind`, `EdgeKind` from Task 1.
- Produces: `Graph::routes_reaching(&self, target: NodeId) -> Vec<NodeId>` and `Graph::ancestors(&self, id: NodeId) -> Vec<NodeId>` (root-first containment chain).

This is the feature that answers the original question — "which API is hit when I work on this function" — so it gets its own task and its own gate.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use crate::{EdgeKind, Graph, Node, NodeKind};

    #[test]
    fn finds_routes_transitively_and_dedupes() {
        let mut g = Graph::new();
        let r1 = g.add_node(Node::new(NodeKind::Route, "POST /chat", "api.py", 1, 2));
        let r2 = g.add_node(Node::new(NodeKind::Route, "POST /batch", "api.py", 4, 5));
        let h = g.add_node(Node::new(NodeKind::Function, "handle", "s.py", 1, 9));
        let deep = g.add_node(Node::new(NodeKind::Function, "normalize", "u.py", 1, 3));
        g.add_edge(r1, h, EdgeKind::Calls);
        g.add_edge(r2, h, EdgeKind::Calls);
        g.add_edge(h, deep, EdgeKind::Calls);

        let mut got = g.routes_reaching(deep);
        got.sort();
        assert_eq!(got, vec![r1, r2]);
        assert_eq!(g.routes_reaching(h), vec![r1, r2]);
        assert!(g.routes_reaching(r1).is_empty(), "a route does not reach itself");
    }

    #[test]
    fn cycles_terminate() {
        let mut g = Graph::new();
        let r = g.add_node(Node::new(NodeKind::Route, "GET /x", "a.py", 1, 2));
        let a = g.add_node(Node::new(NodeKind::Function, "a", "a.py", 3, 4));
        let b = g.add_node(Node::new(NodeKind::Function, "b", "a.py", 5, 6));
        g.add_edge(r, a, EdgeKind::Calls);
        g.add_edge(a, b, EdgeKind::Calls);
        g.add_edge(b, a, EdgeKind::Calls);
        assert_eq!(g.routes_reaching(b), vec![r]);
    }

    #[test]
    fn ancestors_are_root_first() {
        let mut g = Graph::new();
        let p = g.add_node(Node::new(NodeKind::Package, "svc", "svc", 0, 0));
        let m = g.add_node(Node::new(NodeKind::Module, "s.py", "svc/s.py", 0, 0));
        let c = g.add_node(Node::new(NodeKind::Class, "S", "svc/s.py", 1, 20));
        let f = g.add_node(Node::new(NodeKind::Function, "run", "svc/s.py", 2, 5));
        g.add_edge(p, m, EdgeKind::Contains);
        g.add_edge(m, c, EdgeKind::Contains);
        g.add_edge(c, f, EdgeKind::Contains);
        assert_eq!(g.ancestors(f), vec![p, m, c]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codemap-graph reach`
Expected: FAIL — no method `routes_reaching`.

- [ ] **Step 3: Write minimal implementation**

`crates/codemap-graph/src/reach.rs`:

```rust
use crate::{Graph, NodeId, NodeKind};
use std::collections::HashSet;

impl Graph {
    /// API routes that can reach `target` by following Calls edges upward.
    /// Best-effort: only as complete as the resolved call graph (see spec §7).
    pub fn routes_reaching(&self, target: NodeId) -> Vec<NodeId> {
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut stack = vec![target];
        let mut routes = Vec::new();
        while let Some(id) = stack.pop() {
            for caller in self.callers(id) {
                if !seen.insert(caller) { continue; }
                if self.node(caller).kind == NodeKind::Route {
                    routes.push(caller);
                }
                stack.push(caller);
            }
        }
        routes.sort();
        routes.dedup();
        routes
    }

    /// Containment chain from the outermost container down to (not including) `id`.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = Vec::new();
        let mut cur = id;
        let mut guard = 0;
        while let Some(&p) = self.parents_of(cur).first() {
            chain.push(p);
            cur = p;
            guard += 1;
            if guard > 64 { break; }
        }
        chain.reverse();
        chain
    }
}
```

Add `pub mod reach;` to `lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codemap-graph`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy -p codemap-graph -- -D warnings
git add crates/codemap-graph
git commit -m "feat(graph): add route reachability and ancestor chain queries"
```

---

### Task 3: Extractor trait and Python extractor

**Files:**
- Create: `crates/codemap-index/Cargo.toml`
- Create: `crates/codemap-index/src/lib.rs`
- Create: `crates/codemap-index/src/extractor.rs`
- Create: `crates/codemap-index/src/lang/mod.rs`
- Create: `crates/codemap-index/src/lang/python.rs`
- Test: `crates/codemap-index/tests/python.rs`
- Test fixture: `crates/codemap-index/tests/fixtures/sample.py`

**Interfaces:**
- Consumes: `Graph`, `Node`, `NodeKind`, `EdgeKind` from Tasks 1-2.
- Produces:
  - `struct FileFacts { containers: Vec<RawSymbol>, symbols: Vec<RawSymbol>, calls: Vec<RawCall>, routes: Vec<RawRoute>, imports: Vec<String> }`
  - `struct RawSymbol { name: String, parent: Option<String>, line_start: u32, line_end: u32, kind: NodeKind }`
  - `struct RawCall { from_symbol: String, callee: String, resolvable: bool }`
  - `struct RawRoute { method: String, path: String, handler: String, line_start: u32, line_end: u32 }`
  - `trait LanguageExtractor { fn extensions(&self) -> &'static [&'static str]; fn extract(&self, src: &str) -> FileFacts; }`
  - `struct PythonExtractor;` implementing it.

- [ ] **Step 1: Write the failing test**

`crates/codemap-index/tests/fixtures/sample.py`:

```python
from fastapi import APIRouter
from .util import normalize

router = APIRouter()

class ChatService:
    def stream_response(self, req):
        data = normalize(req)
        return self.finish(data)

    def finish(self, data):
        return data

@router.post("/chat/stream")
def chat_stream(req):
    svc = ChatService()
    return svc.stream_response(req)
```

`crates/codemap-index/tests/python.rs`:

```rust
use codemap_graph::NodeKind;
use codemap_index::{lang::python::PythonExtractor, LanguageExtractor};

fn facts() -> codemap_index::FileFacts {
    let src = include_str!("fixtures/sample.py");
    PythonExtractor.extract(src)
}

#[test]
fn extracts_classes_and_functions() {
    let f = facts();
    let classes: Vec<_> = f.containers.iter().filter(|s| s.kind == NodeKind::Class).map(|s| s.name.as_str()).collect();
    assert_eq!(classes, vec!["ChatService"]);

    let mut fns: Vec<_> = f.symbols.iter().map(|s| s.name.as_str()).collect();
    fns.sort();
    assert_eq!(fns, vec!["chat_stream", "finish", "stream_response"]);
}

#[test]
fn methods_record_their_owning_class() {
    let f = facts();
    let m = f.symbols.iter().find(|s| s.name == "stream_response").unwrap();
    assert_eq!(m.parent.as_deref(), Some("ChatService"));
    let top = f.symbols.iter().find(|s| s.name == "chat_stream").unwrap();
    assert_eq!(top.parent, None);
}

#[test]
fn extracts_fastapi_route_with_method_and_path() {
    let f = facts();
    assert_eq!(f.routes.len(), 1);
    let r = &f.routes[0];
    assert_eq!(r.method, "POST");
    assert_eq!(r.path, "/chat/stream");
    assert_eq!(r.handler, "chat_stream");
}

#[test]
fn classifies_calls_by_resolvability() {
    let f = facts();
    // imported bare call -> resolvable
    let n = f.calls.iter().find(|c| c.callee == "normalize").unwrap();
    assert!(n.resolvable);
    // self.finish() -> resolvable within the class
    let s = f.calls.iter().find(|c| c.callee == "finish").unwrap();
    assert!(s.resolvable);
    // svc.stream_response() where svc is a local var -> not resolvable by name alone
    let l = f.calls.iter().find(|c| c.callee == "stream_response").unwrap();
    assert!(!l.resolvable, "local-variable receiver must be marked unresolvable");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codemap-index`
Expected: FAIL — crate `codemap_index` does not exist.

- [ ] **Step 3: Write minimal implementation**

`crates/codemap-index/Cargo.toml`:

```toml
[package]
name = "codemap-index"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
codemap-graph = { path = "../codemap-graph" }
tree-sitter.workspace = true
tree-sitter-python.workspace = true
serde.workspace = true
```

`crates/codemap-index/src/extractor.rs`:

```rust
use codemap_graph::NodeKind;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RawSymbol {
    pub name: String,
    pub parent: Option<String>,
    pub line_start: u32,
    pub line_end: u32,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawCall {
    pub from_symbol: String,
    pub callee: String,
    /// False when the call target cannot be determined from names alone
    /// (a local-variable or chained receiver). Feeds Node::unresolved_calls.
    pub resolvable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawRoute {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub line_start: u32,
    pub line_end: u32,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct FileFacts {
    pub containers: Vec<RawSymbol>,
    pub symbols: Vec<RawSymbol>,
    pub calls: Vec<RawCall>,
    pub routes: Vec<RawRoute>,
    pub imports: Vec<String>,
}

pub trait LanguageExtractor {
    fn extensions(&self) -> &'static [&'static str];
    /// Parse `src` and return facts. MUST NOT retain the syntax tree.
    fn extract(&self, src: &str) -> FileFacts;
}
```

`crates/codemap-index/src/lang/python.rs` — walk the tree with a cursor, tracking the
enclosing class and function so symbols get parents and calls get an owner:

```rust
use crate::extractor::{FileFacts, LanguageExtractor, RawCall, RawRoute, RawSymbol};
use codemap_graph::NodeKind;
use std::collections::HashSet;
use tree_sitter::{Node as TsNode, Parser};

pub struct PythonExtractor;

fn text<'a>(n: TsNode, src: &'a str) -> &'a str { &src[n.byte_range()] }
fn child_name(n: TsNode, src: &str) -> Option<String> {
    n.child_by_field_name("name").map(|c| text(c, src).to_string())
}

impl LanguageExtractor for PythonExtractor {
    fn extensions(&self) -> &'static [&'static str] { &["py"] }

    fn extract(&self, src: &str) -> FileFacts {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_python::LANGUAGE.into()).expect("python grammar");
        let Some(tree) = p.parse(src, None) else { return FileFacts::default() };
        let mut f = FileFacts::default();
        let mut imports: HashSet<String> = HashSet::new();
        collect_imports(tree.root_node(), src, &mut imports);
        f.imports = imports.iter().cloned().collect();
        walk(tree.root_node(), src, None, None, &imports, &mut f);
        f // tree dropped here — never retained
    }
}

fn collect_imports(n: TsNode, src: &str, out: &mut HashSet<String>) {
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        if matches!(ch.kind(), "import_from_statement" | "import_statement") {
            let mut cc = ch.walk();
            for d in ch.children(&mut cc) {
                if d.kind() == "dotted_name" {
                    if let Some(last) = d.child(d.child_count().saturating_sub(1)) {
                        out.insert(text(last, src).to_string());
                    }
                } else if d.kind() == "aliased_import" {
                    if let Some(a) = d.child_by_field_name("alias") {
                        out.insert(text(a, src).to_string());
                    }
                }
            }
        }
        collect_imports(ch, src, out);
    }
}

fn walk(
    n: TsNode, src: &str,
    class: Option<&str>, func: Option<&str>,
    imports: &HashSet<String>, f: &mut FileFacts,
) {
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        match ch.kind() {
            "class_definition" => {
                if let Some(name) = child_name(ch, src) {
                    f.containers.push(RawSymbol {
                        name: name.clone(), parent: class.map(String::from),
                        line_start: ch.start_position().row as u32 + 1,
                        line_end: ch.end_position().row as u32 + 1,
                        kind: NodeKind::Class,
                    });
                    walk(ch, src, Some(&name), None, imports, f);
                    continue;
                }
            }
            "function_definition" => {
                if let Some(name) = child_name(ch, src) {
                    f.symbols.push(RawSymbol {
                        name: name.clone(), parent: class.map(String::from),
                        line_start: ch.start_position().row as u32 + 1,
                        line_end: ch.end_position().row as u32 + 1,
                        kind: NodeKind::Function,
                    });
                    walk(ch, src, class, Some(&name), imports, f);
                    continue;
                }
            }
            "decorated_definition" => {
                if let Some(r) = route_from_decorated(ch, src) { f.routes.push(r); }
                walk(ch, src, class, func, imports, f);
                continue;
            }
            "call" => {
                if let Some(fun) = ch.child_by_field_name("function") {
                    let owner = func.unwrap_or("<module>").to_string();
                    match fun.kind() {
                        "identifier" => f.calls.push(RawCall {
                            from_symbol: owner,
                            callee: text(fun, src).to_string(),
                            resolvable: true,
                        }),
                        "attribute" => {
                            let attr = fun.child_by_field_name("attribute")
                                .map(|a| text(a, src).to_string()).unwrap_or_default();
                            let recv = fun.child_by_field_name("object");
                            let resolvable = match recv {
                                Some(r) if r.kind() == "identifier" => {
                                    let t = text(r, src);
                                    t == "self" || t == "cls" || imports.contains(t)
                                }
                                _ => false,
                            };
                            f.calls.push(RawCall { from_symbol: owner, callee: attr, resolvable });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        walk(ch, src, class, func, imports, f);
    }
}

/// `@router.post("/x")` above a `def handler(...)`.
fn route_from_decorated(n: TsNode, src: &str) -> Option<RawRoute> {
    const METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];
    let mut c = n.walk();
    let mut found: Option<(String, String)> = None;
    for ch in n.children(&mut c) {
        if ch.kind() != "decorator" { continue; }
        let call = ch.child(1)?;
        if call.kind() != "call" { continue; }
        let fun = call.child_by_field_name("function")?;
        if fun.kind() != "attribute" { continue; }
        let m = text(fun.child_by_field_name("attribute")?, src).to_lowercase();
        if !METHODS.contains(&m.as_str()) { continue; }
        let args = call.child_by_field_name("arguments")?;
        let mut ac = args.walk();
        let path = args.children(&mut ac)
            .find(|a| a.kind() == "string")
            .map(|a| text(a, src).trim_matches(['"', '\'']).to_string())?;
        found = Some((m.to_uppercase(), path));
    }
    let (method, path) = found?;
    let mut c2 = n.walk();
    let def = n.children(&mut c2).find(|c| c.kind() == "function_definition")?;
    Some(RawRoute {
        method, path,
        handler: child_name(def, src)?,
        line_start: def.start_position().row as u32 + 1,
        line_end: def.end_position().row as u32 + 1,
    })
}
```

`crates/codemap-index/src/lang/mod.rs`:

```rust
pub mod python;
```

`crates/codemap-index/src/lib.rs`:

```rust
pub mod extractor;
pub mod lang;
pub use extractor::{FileFacts, LanguageExtractor, RawCall, RawRoute, RawSymbol};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codemap-index`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy -p codemap-index -- -D warnings
git add crates/codemap-index
git commit -m "feat(index): add extractor trait and Python extractor with route detection"
```

---

### Task 4: Local-assignment heuristic for Python

**Files:**
- Modify: `crates/codemap-index/src/lang/python.rs`
- Test: `crates/codemap-index/tests/python.rs` (append)

**Interfaces:**
- Consumes: `PythonExtractor`, `RawCall` from Task 3.
- Produces: no new public types. `RawCall.resolvable` becomes true for receivers bound by a same-function `x = ClassName()` assignment, and `RawCall.callee` for those becomes `ClassName.method`.

The spike measured `<local var>.x()` at 17,136 of 23,848 unresolvable Python call sites — 72% of the gap. This task attempts to recover part of it. **If it does not measurably improve the ratio on `core-service`, revert it**; a heuristic that adds complexity without moving the number is not worth carrying.

- [ ] **Step 1: Write the failing test**

Append to `crates/codemap-index/tests/python.rs`:

```rust
#[test]
fn local_var_bound_to_constructor_resolves_to_class_method() {
    let src = r#"
class ChatService:
    def run(self): pass

def handler():
    svc = ChatService()
    return svc.run()
"#;
    let f = PythonExtractor.extract(src);
    let c = f.calls.iter().find(|c| c.callee.ends_with("run") && c.from_symbol == "handler").unwrap();
    assert!(c.resolvable, "x = ClassName() then x.run() should resolve");
    assert_eq!(c.callee, "ChatService.run");
}

#[test]
fn local_var_from_unknown_source_stays_unresolvable() {
    let src = r#"
def handler(dep):
    return dep.run()
"#;
    let f = PythonExtractor.extract(src);
    let c = f.calls.iter().find(|c| c.callee == "run").unwrap();
    assert!(!c.resolvable, "parameter receiver has no known type");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codemap-index local_var`
Expected: FAIL — first test fails, `resolvable` is false and callee is `run`.

- [ ] **Step 3: Write minimal implementation**

In `python.rs`, add a per-function binding table built before walking a function body:

```rust
/// Maps local variable name -> class name for `x = ClassName()` assignments
/// inside one function body. Deliberately shallow: no reassignment tracking,
/// no branches, no attribute targets.
fn local_bindings(func: TsNode, src: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    fn rec(n: TsNode, src: &str, out: &mut std::collections::HashMap<String, String>) {
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            if ch.kind() == "assignment" {
                if let (Some(l), Some(r)) = (ch.child_by_field_name("left"), ch.child_by_field_name("right")) {
                    if l.kind() == "identifier" && r.kind() == "call" {
                        if let Some(f) = r.child_by_field_name("function") {
                            if f.kind() == "identifier" {
                                let cls = text(f, src);
                                if cls.chars().next().is_some_and(|c| c.is_uppercase()) {
                                    out.insert(text(l, src).to_string(), cls.to_string());
                                }
                            }
                        }
                    }
                }
            }
            rec(ch, src, out);
        }
    }
    rec(func, src, &mut out);
    out
}
```

Thread a `bindings: &HashMap<String, String>` parameter through `walk`, populated when
entering a `function_definition` and empty at module level. In the `"attribute"` call arm,
before the existing receiver check:

```rust
let resolvable = match recv {
    Some(r) if r.kind() == "identifier" => {
        let t = text(r, src);
        if let Some(cls) = bindings.get(t) {
            f.calls.push(RawCall {
                from_symbol: owner.clone(),
                callee: format!("{cls}.{attr}"),
                resolvable: true,
            });
            continue;
        }
        t == "self" || t == "cls" || imports.contains(t)
    }
    _ => false,
};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codemap-index`
Expected: PASS, 6 tests.

- [ ] **Step 5: Measure the heuristic against the spike baseline**

Run the extractor over `~/Programming/Convai/core-service` and print the resolvable ratio.
Baseline to beat: **69.6%**. Record the new figure in the commit message. If the gain is
under 2 percentage points, revert this task and note it in the spec's §19.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo clippy -p codemap-index -- -D warnings
git add crates/codemap-index
git commit -m "feat(index): resolve local-variable receivers bound to constructors

Resolvable call ratio on core-service: 69.6% -> <measured>%"
```

---

### Task 5: TypeScript/JavaScript extractor

**Files:**
- Create: `crates/codemap-index/src/lang/typescript.rs`
- Modify: `crates/codemap-index/src/lang/mod.rs`
- Modify: `crates/codemap-index/Cargo.toml` (add `tree-sitter-typescript.workspace = true`)
- Test: `crates/codemap-index/tests/typescript.rs`
- Test fixture: `crates/codemap-index/tests/fixtures/sample.ts`

**Interfaces:**
- Consumes: `LanguageExtractor`, `FileFacts`, `RawSymbol`, `RawCall`, `RawRoute` from Task 3.
- Produces: `struct TypeScriptExtractor;` implementing `LanguageExtractor` with `extensions() -> ["ts", "tsx", "js", "jsx", "mjs"]`.

- [ ] **Step 1: Write the failing test**

`crates/codemap-index/tests/fixtures/sample.ts`:

```typescript
import express from "express";
import { normalize } from "./util";

const app = express();

export class ChatService {
  streamResponse(req: Request) {
    const data = normalize(req);
    return this.finish(data);
  }
  finish(data: unknown) { return data; }
}

app.post("/chat/stream", function chatStream(req, res) {
  const svc = new ChatService();
  return svc.streamResponse(req);
});
```

`crates/codemap-index/tests/typescript.rs`:

```rust
use codemap_graph::NodeKind;
use codemap_index::{lang::typescript::TypeScriptExtractor, LanguageExtractor};

fn facts() -> codemap_index::FileFacts {
    TypeScriptExtractor.extract(include_str!("fixtures/sample.ts"))
}

#[test]
fn extracts_classes_and_methods() {
    let f = facts();
    let classes: Vec<_> = f.containers.iter().filter(|s| s.kind == NodeKind::Class).map(|s| s.name.as_str()).collect();
    assert_eq!(classes, vec!["ChatService"]);
    let m = f.symbols.iter().find(|s| s.name == "streamResponse").unwrap();
    assert_eq!(m.parent.as_deref(), Some("ChatService"));
}

#[test]
fn extracts_express_route() {
    let f = facts();
    assert_eq!(f.routes.len(), 1);
    assert_eq!(f.routes[0].method, "POST");
    assert_eq!(f.routes[0].path, "/chat/stream");
}

#[test]
fn this_and_imported_receivers_resolve_local_vars_do_not() {
    let f = facts();
    assert!(f.calls.iter().find(|c| c.callee == "normalize").unwrap().resolvable);
    assert!(f.calls.iter().find(|c| c.callee == "finish").unwrap().resolvable);
    let l = f.calls.iter().find(|c| c.callee.ends_with("streamResponse") && c.from_symbol != "streamResponse").unwrap();
    assert!(l.resolvable, "const svc = new ChatService() should resolve like Python's heuristic");
    assert_eq!(l.callee, "ChatService.streamResponse");
}

#[test]
fn extensions_cover_js_and_tsx() {
    assert!(TypeScriptExtractor.extensions().contains(&"tsx"));
    assert!(TypeScriptExtractor.extensions().contains(&"js"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codemap-index --test typescript`
Expected: FAIL — module `typescript` not found.

- [ ] **Step 3: Write minimal implementation**

Mirror `python.rs`, substituting the TypeScript node kind names. The mapping is:

| Python node kind | TypeScript node kind |
|---|---|
| `class_definition` | `class_declaration` |
| `function_definition` | `function_declaration`, `method_definition`, `arrow_function` |
| `call` | `call_expression` |
| `attribute` | `member_expression` (field `object` / `property`) |
| `assignment` | `variable_declarator` (fields `name` / `value`) |
| `import_from_statement` | `import_statement` (`import_clause` -> `named_imports` / `identifier`) |
| receiver `self` | receiver `this` |
| constructor call `ClassName()` | `new_expression` with field `constructor` |

Route detection: `app.post("/path", handler)` is a `call_expression` whose function is a
`member_expression` whose property is one of get/post/put/patch/delete, with a `string`
first argument. Handler name comes from a named `function` second argument, or the
identifier if the handler is passed by reference; if anonymous, synthesize
`<anonymous>@line`.

Set `extensions()` to `&["ts", "tsx", "js", "jsx", "mjs"]`. Use
`tree_sitter_typescript::LANGUAGE_TYPESCRIPT` for `.ts`/`.tsx` and the same grammar for
`.js` (it is a superset for our purposes).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codemap-index`
Expected: PASS, 10 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy -p codemap-index -- -D warnings
git add crates/codemap-index
git commit -m "feat(index): add TypeScript/JavaScript extractor with Express route detection"
```

---

### Task 6: Go and Rust extractors

**Files:**
- Create: `crates/codemap-index/src/lang/go.rs`
- Create: `crates/codemap-index/src/lang/rust_lang.rs`
- Modify: `crates/codemap-index/src/lang/mod.rs`
- Modify: `crates/codemap-index/Cargo.toml` (add go and rust grammars)
- Test: `crates/codemap-index/tests/go.rs`, `crates/codemap-index/tests/rust_lang.rs`
- Test fixtures: `crates/codemap-index/tests/fixtures/sample.go`, `sample_rs.txt`

**Interfaces:**
- Consumes: `LanguageExtractor`, `FileFacts` from Task 3.
- Produces: `struct GoExtractor;` (`extensions() -> ["go"]`) and `struct RustExtractor;` (`extensions() -> ["rs"]`).

Both languages are statically typed with explicit imports, so receiver resolution is
better than Python's. Go methods carry an explicit receiver type in the declaration
(`func (s *ChatService) Stream(...)`), which gives exact class attribution for free.
Rust `impl` blocks do the same.

- [ ] **Step 1: Write the failing tests**

`crates/codemap-index/tests/fixtures/sample.go`:

```go
package chat

import "net/http"

type ChatService struct{}

func (s *ChatService) Stream(w http.ResponseWriter, r *http.Request) {
	s.finish(w)
}

func (s *ChatService) finish(w http.ResponseWriter) {}

func Register(mux *http.ServeMux) {
	svc := &ChatService{}
	mux.HandleFunc("/chat/stream", svc.Stream)
}
```

`crates/codemap-index/tests/go.rs`:

```rust
use codemap_graph::NodeKind;
use codemap_index::{lang::go::GoExtractor, LanguageExtractor};

#[test]
fn methods_attribute_to_receiver_type() {
    let f = GoExtractor.extract(include_str!("fixtures/sample.go"));
    let types: Vec<_> = f.containers.iter().filter(|s| s.kind == NodeKind::Class).map(|s| s.name.as_str()).collect();
    assert_eq!(types, vec!["ChatService"]);
    let m = f.symbols.iter().find(|s| s.name == "Stream").unwrap();
    assert_eq!(m.parent.as_deref(), Some("ChatService"), "receiver type is the owner");
}

#[test]
fn receiver_calls_resolve() {
    let f = GoExtractor.extract(include_str!("fixtures/sample.go"));
    assert!(f.calls.iter().find(|c| c.callee.ends_with("finish")).unwrap().resolvable);
}

#[test]
fn detects_handlefunc_route() {
    let f = GoExtractor.extract(include_str!("fixtures/sample.go"));
    assert_eq!(f.routes.len(), 1);
    assert_eq!(f.routes[0].path, "/chat/stream");
    assert_eq!(f.routes[0].method, "ANY", "HandleFunc does not specify a method");
}
```

`crates/codemap-index/tests/fixtures/sample_rs.txt` (`.txt` so cargo does not compile it):

```rust
use axum::{routing::post, Router};

pub struct ChatService;

impl ChatService {
    pub async fn stream(&self) -> String {
        self.finish()
    }
    fn finish(&self) -> String { String::new() }
}

pub fn app() -> Router {
    Router::new().route("/chat/stream", post(handler))
}

async fn handler() -> String { String::new() }
```

`crates/codemap-index/tests/rust_lang.rs`:

```rust
use codemap_graph::NodeKind;
use codemap_index::{lang::rust_lang::RustExtractor, LanguageExtractor};

#[test]
fn impl_blocks_own_their_methods() {
    let f = RustExtractor.extract(include_str!("fixtures/sample_rs.txt"));
    let types: Vec<_> = f.containers.iter().filter(|s| s.kind == NodeKind::Class).map(|s| s.name.as_str()).collect();
    assert!(types.contains(&"ChatService"));
    let m = f.symbols.iter().find(|s| s.name == "stream").unwrap();
    assert_eq!(m.parent.as_deref(), Some("ChatService"));
}

#[test]
fn self_calls_resolve() {
    let f = RustExtractor.extract(include_str!("fixtures/sample_rs.txt"));
    assert!(f.calls.iter().find(|c| c.callee.ends_with("finish")).unwrap().resolvable);
}

#[test]
fn detects_axum_route() {
    let f = RustExtractor.extract(include_str!("fixtures/sample_rs.txt"));
    assert_eq!(f.routes.len(), 1);
    assert_eq!(f.routes[0].method, "POST");
    assert_eq!(f.routes[0].path, "/chat/stream");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p codemap-index --test go --test rust_lang`
Expected: FAIL — modules `go` and `rust_lang` not found.

- [ ] **Step 3: Write minimal implementations**

Go node kinds: `type_declaration` + `type_spec` (struct types -> `NodeKind::Class`),
`method_declaration` (field `receiver` gives the owning type; strip `*` and package
qualifiers), `function_declaration`, `call_expression` with `selector_expression`
(fields `operand` / `field`). A receiver is resolvable when it is the method's own
receiver identifier, a package name from the import block, or a variable bound by
`x := &Type{}` / `x := Type{}` in the same function.

Route detection: `mux.HandleFunc("/path", handler)` and `router.Get("/path", h)` style
(chi, gin, echo all use `.Method("/path", handler)`). `HandleFunc` has no method, so emit
`method: "ANY"`.

Rust node kinds: `struct_item`/`enum_item` -> `NodeKind::Class`, `impl_item` (field `type`
names the owner, and its `declaration_list` children are the methods), `function_item`,
`call_expression`, `method_call_expression` (field `receiver` / `method`). `self` receivers
resolve to the enclosing `impl` type.

Route detection: axum's `.route("/path", post(handler))` — a `method_call_expression`
named `route` whose first argument is a string literal and whose second is a call to
`get`/`post`/`put`/`patch`/`delete`. Also handle actix's `#[get("/path")]` attribute macro
on a `function_item`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p codemap-index`
Expected: PASS, 16 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy -p codemap-index -- -D warnings
git add crates/codemap-index
git commit -m "feat(index): add Go and Rust extractors"
```

---

### Task 7: Repo walker and graph builder

**Files:**
- Create: `crates/codemap-index/src/walker.rs`
- Create: `crates/codemap-index/src/indexer.rs`
- Modify: `crates/codemap-index/src/lib.rs`
- Modify: `crates/codemap-index/Cargo.toml` (add `ignore.workspace = true`)
- Test: `crates/codemap-index/tests/indexer.rs`
- Test fixtures: `crates/codemap-index/tests/fixtures/repo/` (a tiny multi-file repo)

**Interfaces:**
- Consumes: every extractor from Tasks 3-6, `Graph`/`Node`/`NodeKind`/`EdgeKind` from Tasks 1-2.
- Produces:
  - `fn walk_repo(root: &Path) -> Vec<PathBuf>` — respects `.gitignore`, skips `node_modules`, `target`, `.git`, `dist`, `venv`, `.venv`, `__pycache__`.
  - `fn extractor_for(path: &Path) -> Option<Box<dyn LanguageExtractor>>` — dispatches on file extension across all four extractors from Tasks 3-6; returns `None` for unknown extensions so non-source files are skipped without error.
  - `fn index_repo(root: &Path) -> Graph` — eager, whole-repo, trees dropped per file.

Two-pass build: pass one creates all container and symbol nodes and records a
qualified-name -> `NodeId` table; pass two resolves `RawCall.callee` against that table and
emits `Calls` edges, incrementing `Node::unresolved_calls` for every call it cannot place.

- [ ] **Step 1: Write the failing test**

Create `crates/codemap-index/tests/fixtures/repo/api.py`:

```python
from fastapi import APIRouter
from svc import ChatService

router = APIRouter()

@router.post("/chat/stream")
def chat_stream(req, mystery):
    svc = ChatService()
    mystery.run()          # parameter receiver -> unresolvable, must be counted
    return svc.stream(req)
```

Create `crates/codemap-index/tests/fixtures/repo/svc.py`:

```python
class ChatService:
    def stream(self, req):
        return req
```

Create `crates/codemap-index/tests/fixtures/repo/node_modules/junk.js`:

```javascript
module.exports = function ignored() {};
```

`crates/codemap-index/tests/indexer.rs`:

```rust
use codemap_graph::NodeKind;
use codemap_index::indexer::index_repo;
use std::path::Path;

fn g() -> codemap_graph::Graph { index_repo(Path::new("tests/fixtures/repo")) }

#[test]
fn skips_vendored_directories() {
    let graph = g();
    assert!(!graph.nodes().iter().any(|n| n.file.contains("node_modules")),
        "node_modules must never be indexed");
}

#[test]
fn builds_modules_classes_and_functions() {
    let graph = g();
    let kinds = |k: NodeKind| graph.nodes().iter().filter(|n| n.kind == k).count();
    assert_eq!(kinds(NodeKind::Module), 2);
    assert_eq!(kinds(NodeKind::Class), 1);
    assert!(kinds(NodeKind::Function) >= 2);
    assert_eq!(kinds(NodeKind::Route), 1);
}

#[test]
fn route_reaches_the_method_it_calls_across_files() {
    let graph = g();
    let stream = graph.nodes().iter()
        .find(|n| n.kind == NodeKind::Function && n.name == "stream").unwrap();
    let routes = graph.routes_reaching(stream.id);
    assert_eq!(routes.len(), 1, "POST /chat/stream should reach ChatService.stream");
    assert_eq!(graph.node(routes[0]).name, "POST /chat/stream");
}

#[test]
fn unresolved_calls_are_counted_not_dropped() {
    let graph = g();
    // `mystery.run()` in api.py has a parameter receiver with no known type, so it
    // cannot resolve. It must be COUNTED, never silently discarded -- otherwise the
    // graph implies complete call coverage it does not have (spec section 7).
    let caller = graph.nodes().iter()
        .find(|n| n.kind == NodeKind::Function && n.name == "chat_stream").unwrap();
    assert!(caller.unresolved_calls >= 1,
        "unresolvable call must increment the caller's unresolved_calls");

    // And the resolvable call in the same function still produced a real edge.
    assert!(!graph.callees(caller.id).is_empty(),
        "resolvable calls must still yield Calls edges");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codemap-index --test indexer`
Expected: FAIL — module `indexer` not found.

- [ ] **Step 3: Write minimal implementation**

`walker.rs`:

```rust
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

const SKIP: [&str; 7] = ["node_modules", "target", ".git", "dist", "venv", ".venv", "__pycache__"];

pub fn walk_repo(root: &Path) -> Vec<PathBuf> {
    WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|e| !SKIP.contains(&e.file_name().to_string_lossy().as_ref()))
        .build()
        .flatten()
        .map(|e| e.into_path())
        .filter(|p| p.is_file())
        .collect()
}
```

`indexer.rs` — two passes over the file list, using a `HashMap<String, NodeId>` keyed on
qualified name (`module.Class.method`, `module.function`, and a bare-name fallback table
for cross-file resolution). For each file: pick the extractor by extension, extract facts,
create a `Module` node, create `Class` nodes for containers and `Function` nodes for
symbols with `Contains` edges, and create `Route` nodes named `"{METHOD} {path}"` with a
`Calls` edge to their handler. Pass two walks `FileFacts.calls`, looks up each callee first
by qualified name then by bare name, emits a `Calls` edge on a hit, and increments the
caller node's `unresolved_calls` on a miss or when `RawCall.resolvable` is false.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codemap-index`
Expected: PASS, 20 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy -p codemap-index -- -D warnings
git add crates/codemap-index
git commit -m "feat(index): add repo walker and two-pass graph builder"
```

---

### Task 8: CLI

**Files:**
- Create: `crates/codemap-cli/Cargo.toml`
- Create: `crates/codemap-cli/src/main.rs`
- Test: `crates/codemap-cli/tests/cli.rs`

**Interfaces:**
- Consumes: `index_repo` from Task 7, `routes_reaching`/`ancestors` from Task 2.
- Produces: binary `codemap` with subcommands `index <path> [--json]` and `reach <path> <symbol>`.

- [ ] **Step 1: Write the failing test**

```rust
use std::process::Command;

fn run(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_codemap")).args(args).output().expect("run");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn index_prints_a_summary() {
    let s = run(&["index", "../codemap-index/tests/fixtures/repo"]);
    assert!(s.contains("modules"), "got: {s}");
    assert!(s.contains("routes"), "got: {s}");
    assert!(s.contains("resolved"), "must report call-resolution coverage");
}

#[test]
fn index_json_is_parseable_and_has_nodes_and_edges() {
    let s = run(&["index", "../codemap-index/tests/fixtures/repo", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
    assert!(v["nodes"].as_array().unwrap().len() >= 5);
    assert!(v["edges"].as_array().is_some());
}

#[test]
fn reach_names_the_route_that_reaches_a_symbol() {
    let s = run(&["reach", "../codemap-index/tests/fixtures/repo", "stream"]);
    assert!(s.contains("POST /chat/stream"), "got: {s}");
}

#[test]
fn reach_on_unknown_symbol_exits_cleanly() {
    let out = Command::new(env!("CARGO_BIN_EXE_codemap"))
        .args(["reach", "../codemap-index/tests/fixtures/repo", "nope"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codemap-cli`
Expected: FAIL — package `codemap-cli` has no binary.

- [ ] **Step 3: Write minimal implementation**

```rust
use clap::{Parser, Subcommand};
use codemap_graph::NodeKind;
use codemap_index::indexer::index_repo;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "codemap", version)]
struct Cli { #[command(subcommand)] cmd: Cmd }

#[derive(Subcommand)]
enum Cmd {
    /// Index a repository and print a summary
    Index { path: PathBuf, #[arg(long)] json: bool },
    /// List the API routes that can reach a symbol
    Reach { path: PathBuf, symbol: String },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Index { path, json } => {
            let g = index_repo(&path);
            if json {
                println!("{}", serde_json::to_string_pretty(&g).unwrap());
            } else {
                let c = |k: NodeKind| g.nodes().iter().filter(|n| n.kind == k).count();
                let unresolved: u32 = g.nodes().iter().map(|n| n.unresolved_calls).sum();
                let resolved = g.edges().iter().filter(|e| e.kind == codemap_graph::EdgeKind::Calls).count();
                let total = resolved + unresolved as usize;
                let pct = if total == 0 { 100.0 } else { 100.0 * resolved as f64 / total as f64 };
                println!("modules   {}", c(NodeKind::Module));
                println!("classes   {}", c(NodeKind::Class));
                println!("functions {}", c(NodeKind::Function));
                println!("routes    {}", c(NodeKind::Route));
                println!("calls     {resolved} resolved / {total} total ({pct:.1}%)");
            }
            std::process::ExitCode::SUCCESS
        }
        Cmd::Reach { path, symbol } => {
            let g = index_repo(&path);
            let Some(n) = g.nodes().iter().find(|n| n.name == symbol || n.qualified_name == symbol) else {
                eprintln!("symbol not found: {symbol}");
                return std::process::ExitCode::FAILURE;
            };
            let routes = g.routes_reaching(n.id);
            if routes.is_empty() {
                println!("{} is not reachable from any detected route", n.qualified_name);
            } else {
                println!("{} is reachable from:", n.qualified_name);
                for r in routes { println!("  {}", g.node(r).name); }
            }
            std::process::ExitCode::SUCCESS
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test`
Expected: PASS, all 24 tests across the workspace.

- [ ] **Step 5: Validate against the real repo**

```bash
cargo build --release
./target/release/codemap index ~/Programming/Convai/core-service
```

Expected: ~787 modules, ~1416 classes, ~12432 functions, and a call-resolution
percentage at or above the 69.6% spike baseline. Record the output in the commit message.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo clippy --workspace -- -D warnings
git add crates/codemap-cli
git commit -m "feat(cli): add index and reach subcommands"
```
