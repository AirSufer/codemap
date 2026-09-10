use crate::extractor::LanguageExtractor;
use crate::lang::python::PythonExtractor;
use crate::walker::walk_repo;
use codemap_graph::{EdgeKind, Graph, Node, NodeId, NodeKind};
use std::collections::HashMap;
use std::path::Path;

pub fn extractor_for(path: &Path) -> Option<Box<dyn LanguageExtractor>> {
    let ext = path.extension()?.to_str()?;
    for e in all_extractors() {
        if e.extensions().contains(&ext) {
            return Some(e);
        }
    }
    None
}

fn all_extractors() -> Vec<Box<dyn LanguageExtractor>> {
    vec![Box::new(PythonExtractor)]
}

/// Module path used as the qualified-name prefix: `a/b/c.py` -> `a.b.c`.
fn module_path(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let mut s = rel.with_extension("").to_string_lossy().replace('/', ".");
    if let Some(stripped) = s.strip_prefix("./") {
        s = stripped.to_string();
    }
    s
}

/// Lookup tables for pass two. Kept separate from the graph so resolution
/// stays a pure function of names.
#[derive(Default)]
struct SymbolTable {
    by_qualified: HashMap<String, NodeId>,
    /// Bare name -> every node with that name. A name with more than one
    /// candidate is treated as UNRESOLVED rather than guessed at, so the
    /// graph never claims an edge it cannot justify.
    by_bare: HashMap<String, Vec<NodeId>>,
    /// "Class.method" -> node, for the local-binding heuristic's output.
    by_class_method: HashMap<String, NodeId>,
}

impl SymbolTable {
    fn insert(&mut self, qualified: &str, bare: &str, class: Option<&str>, id: NodeId) {
        self.by_qualified.insert(qualified.to_string(), id);
        self.by_bare.entry(bare.to_string()).or_default().push(id);
        if let Some(c) = class {
            self.by_class_method.insert(format!("{c}.{bare}"), id);
        }
    }

    fn resolve(&self, callee: &str) -> Option<NodeId> {
        if let Some(id) = self.by_qualified.get(callee) {
            return Some(*id);
        }
        if callee.contains('.') {
            if let Some(id) = self.by_class_method.get(callee) {
                return Some(*id);
            }
            // Fall back to the final segment.
            let tail = callee.rsplit('.').next()?;
            return self.resolve_bare(tail);
        }
        self.resolve_bare(callee)
    }

    fn resolve_bare(&self, name: &str) -> Option<NodeId> {
        match self.by_bare.get(name) {
            Some(v) if v.len() == 1 => Some(v[0]),
            _ => None, // absent, or ambiguous -> unresolved, deliberately
        }
    }
}

pub fn index_repo(root: &Path) -> Graph {
    let mut g = Graph::new();
    let mut table = SymbolTable::default();
    // (owner node id, callee string, resolvable flag)
    let mut pending: Vec<(NodeId, String, bool)> = Vec::new();

    for file in walk_repo(root) {
        let Some(ex) = extractor_for(&file) else {
            continue;
        };
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        let facts = ex.extract(&src);
        let modpath = module_path(root, &file);
        let file_s = file.to_string_lossy().to_string();
        let line_count = src.lines().count() as u32;

        let mut module = Node::new(NodeKind::Module, &modpath, &file_s, 1, line_count);
        module.qualified_name = modpath.clone();
        let module_id = g.add_node(module);

        // Containers (classes)
        let mut class_ids: HashMap<String, NodeId> = HashMap::new();
        for c in &facts.containers {
            let qualified = format!("{modpath}.{}", c.name);
            let mut n = Node::new(c.kind, &c.name, &file_s, c.line_start, c.line_end);
            n.qualified_name = qualified.clone();
            let id = g.add_node(n);
            g.add_edge(module_id, id, EdgeKind::Contains);
            class_ids.insert(c.name.clone(), id);
            table.insert(&qualified, &c.name, None, id);
        }

        // Symbols (functions and methods)
        let mut symbol_ids: HashMap<String, NodeId> = HashMap::new();
        for s in &facts.symbols {
            let qualified = match &s.parent {
                Some(p) => format!("{modpath}.{p}.{}", s.name),
                None => format!("{modpath}.{}", s.name),
            };
            let mut n = Node::new(s.kind, &s.name, &file_s, s.line_start, s.line_end);
            n.qualified_name = qualified.clone();
            let id = g.add_node(n);
            let parent = s
                .parent
                .as_ref()
                .and_then(|p| class_ids.get(p).copied())
                .unwrap_or(module_id);
            g.add_edge(parent, id, EdgeKind::Contains);
            symbol_ids.insert(s.name.clone(), id);
            table.insert(&qualified, &s.name, s.parent.as_deref(), id);
        }

        // Routes
        for r in &facts.routes {
            let label = format!("{} {}", r.method, r.path);
            let mut n = Node::new(NodeKind::Route, &label, &file_s, r.line_start, r.line_end);
            n.qualified_name = format!("{modpath}::{label}");
            let id = g.add_node(n);
            g.add_edge(module_id, id, EdgeKind::Contains);
            if let Some(h) = symbol_ids.get(&r.handler) {
                g.add_edge(id, *h, EdgeKind::Calls);
            }
        }

        // Defer calls to pass two, when every file's symbols are known.
        for c in &facts.calls {
            let owner = symbol_ids.get(&c.from_symbol).copied().unwrap_or(module_id);
            pending.push((owner, c.callee.clone(), c.resolvable));
        }
    }

    // Pass two: resolve callees against the completed table.
    for (owner, callee, resolvable) in pending {
        let target = if resolvable {
            table.resolve(&callee)
        } else {
            None
        };
        match target {
            Some(t) if t != owner => g.add_edge(owner, t, EdgeKind::Calls),
            Some(_) => {} // self-recursion, not interesting
            None => g.node_mut(owner).unresolved_calls += 1,
        }
    }

    g
}
