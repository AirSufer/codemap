//! Shared daemon state: the graph, the focus engine, and the broadcast channel
//! every connected client listens on.

use crate::event::{ActivityEvent, AgentId};
use crate::focus::{FocusEngine, FocusSnapshot};
use crate::resolver::Resolver;
use codemap_graph::{Graph, NodeId, NodeKind};
use codemap_index::indexer::index_repo;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    // Boxed: the detail payload now carries source and a rendered tree, which
    // would otherwise make every message as large as the largest variant.
    Graph(Box<GraphPayload>),
    Focus(Box<FocusSnapshot>),
    Detail(Box<DetailPayload>),
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphPayload {
    pub nodes: Vec<WireNode>,
    pub edges: Vec<WireEdge>,
    pub root: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WireNode {
    pub id: u32,
    pub kind: String,
    pub name: String,
    pub qualified_name: String,
    pub file: String,
    pub line_start: u32,
    pub line_end: u32,
    pub unresolved_calls: u32,
    pub dep_count: u32,
    /// Containment parent, so the client can build the nested scale levels.
    pub parent: Option<u32>,
    pub child_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct WireEdge {
    pub from: u32,
    pub to: u32,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetailPayload {
    pub id: u32,
    pub qualified_name: String,
    pub file: String,
    pub line_start: u32,
    pub line_end: u32,
    pub source: String,
    pub callers: Vec<u32>,
    pub callees: Vec<u32>,
    pub routes: Vec<u32>,
    pub unresolved_calls: u32,
    pub dependencies: Vec<String>,
    /// Calls from this node that did resolve, for the knows-here panel.
    pub resolved_calls: u32,
    /// Declaration line(s), up to the body.
    pub signature: String,
    /// Docstring or leading comment block, when the code carries one.
    pub doc: String,
    /// For containers, what they hold — packages have no source to quote.
    pub composition: String,
    /// One sentence: what this is and what role it plays.
    pub headline: String,
    /// Git-tree view of how this node connects. Generated once, rendered
    /// verbatim by both the browser panel and the terminal preview.
    pub tree: String,
}

pub struct Inner {
    pub graph: Graph,
    pub focus: FocusEngine,
    pub root: PathBuf,
    /// Files a Tier 1 hook reported recently, so the filesystem watcher does
    /// not double-count the same edit with no attributable actor.
    pub hooked: Vec<(PathBuf, Instant)>,
}

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Mutex<Inner>>,
    pub tx: broadcast::Sender<String>,
}

impl AppState {
    pub fn new(root: &Path) -> Self {
        let graph = index_repo(root);
        let (tx, _) = broadcast::channel(256);
        AppState {
            inner: Arc::new(Mutex::new(Inner {
                graph,
                focus: FocusEngine::new(),
                root: root.to_path_buf(),
                hooked: Vec::new(),
            })),
            tx,
        }
    }

    pub fn graph_payload(&self) -> ServerMsg {
        let g = self.inner.lock().unwrap();
        ServerMsg::Graph(Box::new(build_graph_payload(&g.graph, &g.root)))
    }

    pub fn focus_payload(&self) -> ServerMsg {
        let g = self.inner.lock().unwrap();
        ServerMsg::Focus(Box::new(g.focus.snapshot(&g.graph, Instant::now())))
    }

    pub fn detail(&self, id: u32, width: usize) -> Option<ServerMsg> {
        let g = self.inner.lock().unwrap();
        let nid = NodeId(id);
        if id as usize >= g.graph.len() {
            return None;
        }
        let n = g.graph.node(nid);
        let source = read_span(&n.file, n.line_start, n.line_end);
        use codemap_index::describe;
        let is_container = matches!(n.kind, NodeKind::Package | NodeKind::Module);
        let doc = describe::doc(&n.file, n.line_start, &source);
        // A container has no declaration to quote; its leading comments are
        // documentation, not a signature.
        let signature = if is_container {
            String::new()
        } else {
            describe::signature(&source)
        };
        let composition = if is_container {
            describe::composition(&g.graph, nid)
        } else {
            String::new()
        };
        let headline = describe::headline(&g.graph, nid, &doc);
        Some(ServerMsg::Detail(Box::new(DetailPayload {
            id,
            qualified_name: n.qualified_name.clone(),
            file: n.file.clone(),
            line_start: n.line_start,
            line_end: n.line_end,
            source,
            callers: g.graph.callers(nid).iter().map(|x| x.0).collect(),
            callees: g.graph.callees(nid).iter().map(|x| x.0).collect(),
            routes: g.graph.routes_reaching(nid).iter().map(|x| x.0).collect(),
            unresolved_calls: n.unresolved_calls,
            dependencies: n.dependencies.clone(),
            resolved_calls: g.graph.callees(nid).len() as u32,
            signature,
            doc,
            composition,
            headline,
            tree: describe::tree(&g.graph, nid, width),
        })))
    }

    /// Ingest one activity event: resolve it to a node and update focus.
    pub fn ingest(&self, ev: &ActivityEvent) -> bool {
        let mut g = self.inner.lock().unwrap();
        let Some(node) = Resolver::resolve(&g.graph, ev) else {
            return false;
        };
        let agent = ev.agent;
        let now = Instant::now();
        if agent != AgentId::Unknown {
            g.hooked
                .retain(|(_, t)| now.duration_since(*t) < crate::watch::HOOK_SUPPRESSION);
            g.hooked.push((ev.path.clone(), now));
        }
        g.focus.touch(node, agent, now);
        true
    }

    /// True when a Tier 1 hook reported this file within the suppression window.
    pub fn recently_hooked(&self, path: &Path, now: Instant) -> bool {
        let g = self.inner.lock().unwrap();
        g.hooked.iter().any(|(p, t)| {
            now.duration_since(*t) < crate::watch::HOOK_SUPPRESSION
                && (p == path || p.file_name() == path.file_name())
        })
    }

    pub fn reindex_file(&self, _path: &Path) {
        // Full reindex: measured at 805ms for 787 files, which is acceptable
        // for now. Incremental single-file reindex is the obvious optimization.
        let mut g = self.inner.lock().unwrap();
        let root = g.root.clone();
        g.graph = index_repo(&root);
    }

    pub fn broadcast(&self, msg: &ServerMsg) {
        if let Ok(s) = serde_json::to_string(msg) {
            let _ = self.tx.send(s);
        }
    }

    pub fn set_following(&self, on: bool) {
        let mut g = self.inner.lock().unwrap();
        if on {
            g.focus.attach()
        } else {
            g.focus.detach()
        }
    }
}

fn read_span(file: &str, start: u32, end: u32) -> String {
    let Ok(text) = std::fs::read_to_string(file) else {
        return String::new();
    };
    text.lines()
        .skip(start.saturating_sub(1) as usize)
        .take((end.saturating_sub(start) + 1).min(400) as usize)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn build_graph_payload(graph: &Graph, root: &Path) -> GraphPayload {
    let mut parent = vec![None; graph.len()];
    let mut child_count = vec![0u32; graph.len()];
    for e in graph.edges() {
        if e.kind == codemap_graph::EdgeKind::Contains {
            parent[e.to.0 as usize] = Some(e.from.0);
            child_count[e.from.0 as usize] += 1;
        }
    }
    let nodes = graph
        .nodes()
        .iter()
        .map(|n| WireNode {
            id: n.id.0,
            kind: kind_name(n.kind).to_string(),
            name: n.name.clone(),
            qualified_name: n.qualified_name.clone(),
            file: n.file.clone(),
            line_start: n.line_start,
            line_end: n.line_end,
            unresolved_calls: n.unresolved_calls,
            dep_count: n.dependencies.len() as u32,
            parent: parent[n.id.0 as usize],
            child_count: child_count[n.id.0 as usize],
        })
        .collect();
    let edges = graph
        .edges()
        .iter()
        .map(|e| WireEdge {
            from: e.from.0,
            to: e.to.0,
            kind: format!("{:?}", e.kind).to_lowercase(),
        })
        .collect();
    GraphPayload {
        nodes,
        edges,
        root: root.display().to_string(),
    }
}

fn kind_name(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Package => "package",
        NodeKind::Module => "module",
        NodeKind::Class => "class",
        NodeKind::Function => "function",
        NodeKind::Route => "route",
    }
}

/// Socket path for a repo. Derived from the path so each repo gets its own
/// daemon, and so a hook can check "is a daemon running here?" with one stat().
pub fn socket_path(root: &Path) -> PathBuf {
    let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut h: u64 = 1469598103934665603;
    for b in canon.display().to_string().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    let dir = dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("codemap");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{h:016x}.sock"))
}

pub fn agent_from_str(s: &str) -> AgentId {
    AgentId::parse(s)
}
