use crate::extractor::LanguageExtractor;
use crate::lang::go::GoExtractor;
use crate::lang::python::PythonExtractor;
use crate::lang::rust_lang::RustExtractor;
use crate::lang::typescript::TypeScriptExtractor;
use crate::walker::walk_repo_opts;
use codemap_graph::{EdgeKind, Graph, Node, NodeId, NodeKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn extractor_for(path: &Path) -> Option<Box<dyn LanguageExtractor>> {
    let ext = path.extension()?.to_str()?;
    all_extractors()
        .into_iter()
        .find(|e| e.extensions().contains(&ext))
}

fn all_extractors() -> Vec<Box<dyn LanguageExtractor>> {
    vec![
        Box::new(PythonExtractor),
        Box::new(TypeScriptExtractor),
        Box::new(GoExtractor),
        Box::new(RustExtractor),
    ]
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

    /// True when the name IS indexed but matches more than one symbol.
    /// Distinguishes "we could not choose" from "it lives outside this repo".
    fn is_ambiguous(&self, callee: &str) -> bool {
        let tail = callee.rsplit('.').next().unwrap_or(callee);
        self.by_bare.get(tail).map(|v| v.len() > 1).unwrap_or(false)
    }

    fn resolve_bare(&self, name: &str) -> Option<NodeId> {
        match self.by_bare.get(name) {
            Some(v) if v.len() == 1 => Some(v[0]),
            _ => None, // absent, or ambiguous -> unresolved, deliberately
        }
    }
}

/// Why a call site produced no edge. These are very different failures and
/// must not be reported as one number: an unindexed callee is usually stdlib
/// or a third-party package, which is expected and harmless, whereas an
/// ambiguous or undeterminable callee is real missing coverage.
#[derive(Debug, Default, Clone, Copy)]
pub struct IndexStats {
    pub calls_total: usize,
    /// Resolved to a node in this repo.
    pub resolved: usize,
    /// Extractor could not determine the callee name at all (see spec section 7).
    pub undeterminable: usize,
    /// Name known, but no symbol with that name is indexed -- almost always
    /// stdlib or a third-party dependency, i.e. legitimately outside the graph.
    pub external: usize,
    /// Name known and indexed, but more than one candidate matched. Treated as
    /// unresolved rather than guessed.
    pub ambiguous: usize,
}

impl IndexStats {
    /// Share of call sites that resolved, counting only calls that could
    /// plausibly target this repo (i.e. excluding external/stdlib calls).
    pub fn internal_resolution_pct(&self) -> f64 {
        let denom = self.resolved + self.undeterminable + self.ambiguous;
        if denom == 0 {
            return 100.0;
        }
        100.0 * self.resolved as f64 / denom as f64
    }
}

pub fn index_repo(root: &Path) -> Graph {
    index_repo_with_stats(root).0
}

pub fn index_repo_with_stats(root: &Path) -> (Graph, IndexStats) {
    index_repo_opts(root, false)
}

/// `include_all` keeps tests, migrations and generated code in the graph.
pub fn index_repo_opts(root: &Path, include_all: bool) -> (Graph, IndexStats) {
    let mut g = Graph::new();
    let mut table = SymbolTable::default();
    // (owner node id, callee string, resolvable flag)
    let mut pending: Vec<(NodeId, String, bool, u32, u32)> = Vec::new();

    // Directory nodes, so the top level shows a handful of packages rather
    // than every file in the repo flat (spec section 4, level L0/L1).
    let mut packages: HashMap<PathBuf, NodeId> = HashMap::new();

    for file in walk_repo_opts(root, include_all) {
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

        // Display name is the file, not the dotted path: an outline row reading
        // "resolver.rs" is legible where "crates.codemap-daemon.src.resolver"
        // is not. The dotted form stays as the qualified name.
        let base = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| modpath.clone());
        let mut module = Node::new(NodeKind::Module, &base, &file_s, 1, line_count);
        module.qualified_name = modpath.clone();
        let module_id = g.add_node(module);
        if let Some(dir) = file.parent() {
            if let Some(pkg) = ensure_package(&mut g, &mut packages, root, dir) {
                g.add_edge(pkg, module_id, EdgeKind::Contains);
            }
        }

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
            pending.push((owner, c.callee.clone(), c.resolvable, c.line, c.col));
        }
    }

    // Pass two: resolve callees against the completed table.
    let mut stats = IndexStats::default();
    for (owner, callee, resolvable, line, col) in pending {
        stats.calls_total += 1;
        if !resolvable {
            stats.undeterminable += 1;
            g.node_mut(owner).unresolved_calls += 1;
            continue;
        }
        match table.resolve(&callee) {
            Some(t) if t != owner => {
                g.add_call_edge(owner, t, line, col);
                stats.resolved += 1;
            }
            Some(_) => stats.resolved += 1, // self-recursion; counted, no edge
            None => {
                if table.is_ambiguous(&callee) {
                    stats.ambiguous += 1;
                    g.node_mut(owner).unresolved_calls += 1;
                } else {
                    stats.external += 1;
                    let short = callee.rsplit('.').next().unwrap_or(&callee).to_string();
                    if !is_language_builtin(&short) {
                        let n = g.node_mut(owner);
                        if !n.dependencies.contains(&short) && n.dependencies.len() < 24 {
                            n.dependencies.push(short);
                        }
                    }
                }
            }
        }
    }

    (g, stats)
}

/// Creates Package nodes for `dir` and every ancestor up to `root`, linking
/// each into its parent. Memoized so each directory yields exactly one node.
/// Without these the top level renders every file in the repo flat, which is
/// the hairball the nested-scale model exists to avoid (spec section 4).
fn ensure_package(
    g: &mut Graph,
    seen: &mut HashMap<PathBuf, NodeId>,
    root: &Path,
    dir: &Path,
) -> Option<NodeId> {
    let rel = dir.strip_prefix(root).ok()?;
    if rel.as_os_str().is_empty() {
        return None; // the repo root itself is the implicit top level
    }
    if let Some(id) = seen.get(dir) {
        return Some(*id);
    }
    let name = dir.file_name()?.to_string_lossy().to_string();
    let mut n = Node::new(NodeKind::Package, &name, &dir.to_string_lossy(), 0, 0);
    n.qualified_name = rel.to_string_lossy().replace('/', ".");
    let id = g.add_node(n);
    seen.insert(dir.to_path_buf(), id);
    if let Some(parent) = dir.parent() {
        if let Some(pid) = ensure_package(g, seen, root, parent) {
            g.add_edge(pid, id, EdgeKind::Contains);
        }
    }
    Some(id)
}

/// Constructors and builtins that are part of the language, not a dependency.
/// Listing them as "dependencies" is noise, which is the whole complaint the
/// default exclusions exist to answer.
fn is_language_builtin(name: &str) -> bool {
    const BUILTINS: &[&str] = &[
        // Rust
        "Some",
        "None",
        "Ok",
        "Err",
        "Vec",
        "String",
        "Box",
        "Rc",
        "Arc",
        "new",
        "from",
        "into",
        "clone",
        "to_string",
        "unwrap",
        "expect",
        "format",
        // Python
        "len",
        "str",
        "int",
        "float",
        "bool",
        "dict",
        "list",
        "set",
        "tuple",
        "print",
        "range",
        "isinstance",
        "getattr",
        "setattr",
        "super",
        "open",
        "enumerate",
        "zip",
        "sorted",
        "type",
        "repr",
        "hasattr",
        // JS / TS / Go
        "console",
        "require",
        "Object",
        "Array",
        "Promise",
        "JSON",
        "make",
        "append",
    ];
    BUILTINS.contains(&name)
}
