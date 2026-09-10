//! Turns "something happened in file X at lines A-B" into "that was `Class.method`".
//!
//! Degrades gracefully by design: an unresolvable event falls back to the file
//! node, then to nothing. It never errors — a miss just means a coarser highlight.

use crate::event::ActivityEvent;
use codemap_graph::{Graph, NodeId, NodeKind};
use std::path::Path;

pub struct Resolver;

impl Resolver {
    /// Innermost node containing the event's line range, preferring functions
    /// over classes over modules.
    pub fn resolve(graph: &Graph, ev: &ActivityEvent) -> Option<NodeId> {
        let candidates: Vec<&codemap_graph::Node> = graph
            .nodes()
            .iter()
            .filter(|n| same_file(&n.file, &ev.path))
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let Some(range) = ev.range else {
            // No line information: fall back to the file's module node.
            return candidates
                .iter()
                .find(|n| n.kind == NodeKind::Module)
                .map(|n| n.id)
                .or_else(|| candidates.first().map(|n| n.id));
        };
        // Smallest span that contains the range wins; ties broken by specificity.
        let mut best: Option<(&codemap_graph::Node, u32)> = None;
        for n in &candidates {
            if n.kind == NodeKind::Module {
                continue;
            }
            if n.line_start <= range.end && n.line_end >= range.start {
                let span = n.line_end.saturating_sub(n.line_start);
                match best {
                    Some((_, bs)) if bs <= span => {}
                    _ => best = Some((n, span)),
                }
            }
        }
        best.map(|(n, _)| n.id).or_else(|| {
            candidates
                .iter()
                .find(|n| n.kind == NodeKind::Module)
                .map(|n| n.id)
        })
    }
}

/// Paths may arrive absolute from a hook and relative from the index, or vice
/// versa. Compare from the right so either form matches.
fn same_file(indexed: &str, event: &Path) -> bool {
    let a = Path::new(indexed);
    if a == event {
        return true;
    }
    let ac: Vec<_> = a.components().collect();
    let bc: Vec<_> = event.components().collect();
    if ac.is_empty() || bc.is_empty() {
        return false;
    }
    let n = ac.len().min(bc.len());
    ac[ac.len() - n..] == bc[bc.len() - n..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{AgentId, EventKind, LineRange};
    use codemap_graph::Node;
    use std::path::PathBuf;

    fn fixture() -> (Graph, NodeId, NodeId, NodeId) {
        let mut g = Graph::new();
        let m = g.add_node(Node::new(NodeKind::Module, "svc", "/repo/svc.py", 1, 100));
        let c = g.add_node(Node::new(NodeKind::Class, "S", "/repo/svc.py", 10, 60));
        let f = g.add_node(Node::new(NodeKind::Function, "run", "/repo/svc.py", 20, 30));
        (g, m, c, f)
    }

    fn ev(path: &str, range: Option<(u32, u32)>) -> ActivityEvent {
        ActivityEvent {
            kind: EventKind::Write,
            path: PathBuf::from(path),
            range: range.map(|(start, end)| LineRange { start, end }),
            agent: AgentId::ClaudeCode,
        }
    }

    #[test]
    fn picks_the_innermost_containing_symbol() {
        let (g, _m, _c, f) = fixture();
        assert_eq!(
            Resolver::resolve(&g, &ev("/repo/svc.py", Some((22, 25)))),
            Some(f)
        );
    }

    #[test]
    fn falls_back_to_the_class_when_between_methods() {
        let (g, _m, c, _f) = fixture();
        assert_eq!(
            Resolver::resolve(&g, &ev("/repo/svc.py", Some((45, 46)))),
            Some(c)
        );
    }

    #[test]
    fn falls_back_to_the_module_without_a_range() {
        let (g, m, _c, _f) = fixture();
        assert_eq!(Resolver::resolve(&g, &ev("/repo/svc.py", None)), Some(m));
    }

    #[test]
    fn matches_relative_and_absolute_paths() {
        let (g, _m, _c, f) = fixture();
        assert_eq!(
            Resolver::resolve(&g, &ev("svc.py", Some((21, 22)))),
            Some(f)
        );
    }

    #[test]
    fn unknown_file_resolves_to_nothing_rather_than_erroring() {
        let (g, ..) = fixture();
        assert!(Resolver::resolve(&g, &ev("/other/zzz.py", Some((1, 2)))).is_none());
    }
}
