//! Turning a node into something a human can read: what it is, what it holds,
//! what it depends on. Shared so the browser panel and the terminal preview
//! never drift apart.

use codemap_graph::{Graph, NodeId, NodeKind};
use std::path::{Path, PathBuf};

/// Nearest ancestor containing `.git`, falling back to the directory itself.
/// Pointing the tool at `src/` should map the project, not the subtree.
pub fn repo_root(start: &Path) -> PathBuf {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut cur: &Path = &start;
    loop {
        if cur.join(".git").exists() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return start,
        }
    }
}

/// Declaration line(s), up to the body.
pub fn signature(source: &str) -> String {
    let mut out = Vec::new();
    for l in source.lines().take(8) {
        let t = l.trim();
        out.push(t.to_string());
        if t.contains('{') || t.ends_with(':') {
            break;
        }
    }
    out.join(" ")
        .trim_end_matches('{')
        .trim()
        .chars()
        .take(300)
        .collect()
}

/// Docstring or leading comment block, whichever the language uses.
pub fn doc(file: &str, line_start: u32, source: &str) -> String {
    let mut doc = String::new();
    let body: Vec<&str> = source.lines().skip(1).collect();
    if let Some(first) = body.iter().find(|l| !l.trim().is_empty()) {
        let t = first.trim();
        for q in ["\"\"\"", "'''"] {
            if let Some(rest) = t.strip_prefix(q) {
                if let Some(end) = rest.find(q) {
                    doc = rest[..end].trim().to_string();
                } else {
                    let mut acc = vec![rest.to_string()];
                    for l in body.iter().skip(1) {
                        if let Some(cut) = l.find(q) {
                            acc.push(l[..cut].to_string());
                            break;
                        }
                        acc.push(l.trim().to_string());
                    }
                    doc = acc.join(" ").trim().to_string();
                }
                break;
            }
        }
    }
    if doc.is_empty() {
        if let Ok(text) = std::fs::read_to_string(file) {
            let lines: Vec<&str> = text.lines().collect();
            // A module's own doc sits at the top of the file (//! or a leading
            // block); a symbol's sits immediately above its declaration.
            if line_start <= 1 {
                let mut acc = Vec::new();
                for l in lines.iter().take(30) {
                    let t = l.trim();
                    match t
                        .strip_prefix("//!")
                        .or_else(|| t.strip_prefix("///"))
                        .or_else(|| t.strip_prefix("//"))
                        .or_else(|| t.strip_prefix("#"))
                    {
                        Some(r) if !t.starts_with("#!") => acc.push(r.trim().to_string()),
                        _ if t.is_empty() && acc.is_empty() => {}
                        _ if t.starts_with("\"\"\"") => {
                            acc.push(t.trim_matches('"').trim().to_string())
                        }
                        _ => break,
                    }
                }
                doc = acc.join(" ").trim().to_string();
            } else {
                let mut acc = Vec::new();
                let mut i = line_start as usize - 1;
                while i > 0 {
                    let t = lines[i - 1].trim();
                    match t
                        .strip_prefix("///")
                        .or_else(|| t.strip_prefix("//!"))
                        .or_else(|| t.strip_prefix("//"))
                        .or_else(|| t.strip_prefix("*"))
                        .or_else(|| t.strip_prefix("#"))
                    {
                        Some(r) if !t.starts_with("#!") => acc.push(r.trim().to_string()),
                        _ => break,
                    }
                    i -= 1;
                }
                acc.reverse();
                doc = acc.join(" ").trim().to_string();
            }
        }
    }
    if doc.len() > 700 {
        doc.truncate(700);
        doc.push('\u{2026}');
    }
    doc
}

/// "8 modules · 24 classes · 60 functions" — what a container holds.
pub fn composition(graph: &Graph, id: NodeId) -> String {
    let mut c = [0usize; 5];
    let mut stack = vec![id];
    let mut guard = 0;
    while let Some(x) = stack.pop() {
        guard += 1;
        if guard > 40000 {
            break;
        }
        for ch in graph.children(x) {
            match graph.node(ch).kind {
                NodeKind::Package => c[0] += 1,
                NodeKind::Module => c[1] += 1,
                NodeKind::Class => c[2] += 1,
                NodeKind::Function => c[3] += 1,
                NodeKind::Route => c[4] += 1,
            }
            stack.push(ch);
        }
    }
    let l = |n: usize, one: &str, many: &str| {
        (n > 0).then(|| format!("{n} {}", if n == 1 { one } else { many }))
    };
    [
        l(c[0], "package", "packages"),
        l(c[1], "module", "modules"),
        l(c[2], "class", "classes"),
        l(c[3], "function", "functions"),
        l(c[4], "route", "routes"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

/// Symbols inside `id` with the most callers — the parts of a file other code
/// actually leans on.
pub fn busiest(graph: &Graph, id: NodeId, n: usize) -> Vec<(NodeId, usize)> {
    let mut out: Vec<(NodeId, usize)> = Vec::new();
    let mut stack = vec![id];
    let mut guard = 0;
    while let Some(x) = stack.pop() {
        guard += 1;
        if guard > 40000 {
            break;
        }
        for ch in graph.children(x) {
            let k = graph.node(ch).kind;
            if matches!(k, NodeKind::Function | NodeKind::Class) {
                let c = graph.callers(ch).len();
                if c > 0 {
                    out.push((ch, c));
                }
            }
            stack.push(ch);
        }
    }
    out.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
    out.truncate(n);
    out
}

/// Routes declared inside `id`.
pub fn routes_within(graph: &Graph, id: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![id];
    let mut guard = 0;
    while let Some(x) = stack.pop() {
        guard += 1;
        if guard > 40000 {
            break;
        }
        for ch in graph.children(x) {
            if graph.node(ch).kind == NodeKind::Route {
                out.push(ch);
            }
            stack.push(ch);
        }
    }
    out
}

/// Every external name used anywhere inside `id`, most common first.
pub fn deps_within(graph: &Graph, id: NodeId, n: usize) -> Vec<String> {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut stack = vec![id];
    let mut guard = 0;
    while let Some(x) = stack.pop() {
        guard += 1;
        if guard > 40000 {
            break;
        }
        for d in &graph.node(x).dependencies {
            *counts.entry(d.as_str()).or_default() += 1;
        }
        for ch in graph.children(x) {
            stack.push(ch);
        }
    }
    let mut v: Vec<(&str, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.into_iter().take(n).map(|(k, _)| k.to_string()).collect()
}

/// Reads a line span from a file, clamped.
pub fn read_span(file: &str, start: u32, end: u32, max: usize) -> String {
    let Ok(text) = std::fs::read_to_string(file) else {
        return String::new();
    };
    text.lines()
        .skip(start.saturating_sub(1) as usize)
        .take((end.saturating_sub(start) as usize + 1).min(max))
        .collect::<Vec<_>>()
        .join("\n")
}
