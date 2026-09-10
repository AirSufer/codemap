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

/* ================= teaching view ================= */

/// One sentence: what this thing is and what role it plays. Falls back to
/// structure when the code carries no doc comment.
pub fn headline(graph: &Graph, id: NodeId, doc: &str) -> String {
    let n = graph.node(id);
    if !doc.is_empty() {
        let first = doc
            .split_terminator(". ")
            .next()
            .unwrap_or(doc)
            .trim()
            .to_string();
        return if first.ends_with('.') {
            first
        } else {
            format!("{first}.")
        };
    }
    match n.kind {
        NodeKind::Module | NodeKind::Package => {
            let c = composition(graph, id);
            if c.is_empty() {
                format!("{} — no symbols indexed.", n.name)
            } else {
                format!("{} — holds {}.", n.name, c)
            }
        }
        NodeKind::Route => format!("{} — an HTTP entry point.", n.name),
        NodeKind::Class => format!(
            "{} — a type with {} members.",
            n.name,
            graph.children(id).len()
        ),
        NodeKind::Function => {
            let inn = graph.callers(id).len();
            let out = graph.callees(id).len();
            format!("{} — called from {inn}, calls out to {out}.", n.name)
        }
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('\u{2026}');
        t
    }
}

/// A git-tree view of what a node is and how it connects.
///
/// Tree connectors rather than boxes-and-arrows: it reads like `tree` or
/// `git log --graph`, which is a shape people already parse without effort.
/// The load-bearing caveat — call sites that could not be resolved — is a
/// branch of the tree, not a footnote, because it is what a reader must not
/// miss when deciding whether the picture is complete.
pub fn tree(graph: &Graph, id: NodeId, width: usize) -> String {
    let n = graph.node(id);
    let w = width.clamp(40, 110);
    let is_container = matches!(n.kind, NodeKind::Module | NodeKind::Package);
    let mut out: Vec<String> = Vec::new();

    let loc = |x: NodeId| -> String {
        let m = graph.node(x);
        format!(
            "{}:{}",
            m.file.rsplit('/').next().unwrap_or(""),
            m.line_start
        )
    };

    // header
    out.push(format!(
        "{}  {}",
        n.name,
        if is_container {
            composition(graph, id)
        } else {
            format!(
                "{} \u{b7} {}",
                match n.kind {
                    NodeKind::Function => "fn",
                    NodeKind::Class => "type",
                    NodeKind::Route => "route",
                    _ => "",
                },
                loc(id)
            )
        }
    ));

    // Sections are built first so the last one can use the closing connector.
    struct Sec {
        title: String,
        rows: Vec<(String, String)>,
        empty: String,
    }
    let mut secs: Vec<Sec> = Vec::new();

    if is_container {
        let entries = busiest(graph, id, 6);
        let mut inbound: Vec<(String, String)> = Vec::new();
        for (sym, _) in &entries {
            for c in graph.callers(*sym) {
                if graph.node(c).file != n.file && inbound.len() < 5 {
                    inbound.push((graph.node(c).name.clone(), loc(c)));
                }
            }
        }
        inbound.dedup_by(|a, b| a.0 == b.0);
        secs.push(Sec {
            title: "reached from".into(),
            rows: inbound,
            empty: "nothing outside this file calls into it".into(),
        });

        let rows: Vec<(String, String)> = anatomy(graph, id, 8)
            .into_iter()
            .map(|r| {
                (
                    format!("{}  :{}", r.name, r.line),
                    if r.fan_in > 0 {
                        format!("\u{2190}{}", r.fan_in)
                    } else {
                        String::new()
                    },
                )
            })
            .collect();
        secs.push(Sec {
            title: "holds".into(),
            rows,
            empty: "no symbols indexed".into(),
        });

        let deps = deps_within(graph, id, 8);
        secs.push(Sec {
            title: "uses".into(),
            rows: deps.into_iter().map(|d| (d, String::new())).collect(),
            empty: "stdlib and this repo only".into(),
        });
    } else {
        secs.push(Sec {
            title: "called by".into(),
            rows: graph
                .callers(id)
                .into_iter()
                .take(6)
                .map(|c| (graph.node(c).name.clone(), loc(c)))
                .collect(),
            empty: "nothing indexed calls this".into(),
        });
        secs.push(Sec {
            title: "calls".into(),
            rows: graph
                .callees(id)
                .into_iter()
                .take(6)
                .map(|c| (graph.node(c).name.clone(), loc(c)))
                .collect(),
            empty: "calls nothing indexed".into(),
        });
        let rts = graph.routes_reaching(id);
        secs.push(Sec {
            title: "reached from".into(),
            rows: rts
                .into_iter()
                .take(4)
                .map(|r| (graph.node(r).name.clone(), loc(r)))
                .collect(),
            empty: "no route found \u{2014} unknown, not unreachable".into(),
        });
        if !n.dependencies.is_empty() {
            secs.push(Sec {
                title: "uses".into(),
                rows: n
                    .dependencies
                    .iter()
                    .take(6)
                    .map(|d| (d.clone(), String::new()))
                    .collect(),
                empty: String::new(),
            });
        }
    }

    let unresolved = if is_container {
        let mut t = 0;
        let mut stack = vec![id];
        while let Some(x) = stack.pop() {
            t += graph.node(x).unresolved_calls;
            for c in graph.children(x) {
                stack.push(c);
            }
        }
        t
    } else {
        n.unresolved_calls
    };

    let total_secs = secs.len() + usize::from(unresolved > 0);
    for (i, sec) in secs.iter().enumerate() {
        let last_sec = i + 1 == total_secs;
        let (branch, spine) = if last_sec {
            ("\u{2514}\u{2500} ", "   ")
        } else {
            ("\u{251c}\u{2500} ", "\u{2502}  ")
        };
        out.push(format!("{branch}{}", sec.title));
        if sec.rows.is_empty() {
            if !sec.empty.is_empty() {
                out.push(format!("{spine}\u{2514}\u{2500} {}", sec.empty));
            }
        } else {
            let lastrow = sec.rows.len() - 1;
            let namew = sec
                .rows
                .iter()
                .map(|(a, _)| a.chars().count())
                .max()
                .unwrap_or(0)
                .min(w.saturating_sub(18));
            for (j, (a, b)) in sec.rows.iter().enumerate() {
                let c = if j == lastrow {
                    "\u{2514}\u{2500} "
                } else {
                    "\u{251c}\u{2500} "
                };
                let a = trunc(a, namew);
                if b.is_empty() {
                    out.push(format!("{spine}{c}{a}"));
                } else {
                    out.push(format!("{spine}{c}{a:<namew$}  {b}"));
                }
            }
        }
    }

    if unresolved > 0 {
        out.push(format!(
            "\u{2514}\u{2500} \u{26a0} {unresolved} call site{} unresolved",
            if unresolved > 1 { "s" } else { "" }
        ));
        out.push("   \u{2514}\u{2500} a caller or callee may exist that is not drawn here".into());
    }
    out.join("\n")
}

/// One row per symbol: what it is, where it lives, how many things lean on it.
pub struct AnatomyRow {
    pub id: NodeId,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub fan_in: usize,
    pub note: String,
}

pub fn anatomy(graph: &Graph, id: NodeId, limit: usize) -> Vec<AnatomyRow> {
    let mut kids: Vec<NodeId> = graph.children(id);
    // A class's methods matter as much as the class, so pull one level deeper.
    let mut extra = Vec::new();
    for &k in &kids {
        if graph.node(k).kind == NodeKind::Class {
            extra.extend(graph.children(k));
        }
    }
    kids.extend(extra);
    let mut rows: Vec<AnatomyRow> = kids
        .into_iter()
        .map(|k| {
            let n = graph.node(k);
            let fan_in = graph.callers(k).len();
            AnatomyRow {
                id: k,
                name: n.name.clone(),
                kind: format!("{:?}", n.kind).to_lowercase(),
                line: n.line_start,
                fan_in,
                note: if n.unresolved_calls > 0 {
                    format!("{} unresolved", n.unresolved_calls)
                } else {
                    String::new()
                },
            }
        })
        .collect();
    rows.sort_by_key(|r| (std::cmp::Reverse(r.fan_in), r.line));
    rows.truncate(limit);
    rows
}
