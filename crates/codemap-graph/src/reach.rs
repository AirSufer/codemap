use crate::{Graph, NodeId, NodeKind};
use std::collections::HashSet;

impl Graph {
    /// API routes that can reach `target` by following Calls edges upward.
    ///
    /// Best-effort: only as complete as the resolved call graph. Roughly 70% of
    /// Python call sites resolve without type inference (see spec section 7), so
    /// an empty result means "no route found", never "no route exists".
    pub fn routes_reaching(&self, target: NodeId) -> Vec<NodeId> {
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut stack = vec![target];
        let mut routes = Vec::new();
        while let Some(id) = stack.pop() {
            for caller in self.callers(id) {
                if !seen.insert(caller) {
                    continue;
                }
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
    /// This is the chain the UI lights up so activity stays visible at every scale.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = Vec::new();
        let mut cur = id;
        let mut guard = 0;
        while let Some(&p) = self.parents_of(cur).first() {
            chain.push(p);
            cur = p;
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        chain.reverse();
        chain
    }
}

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
        assert!(
            g.routes_reaching(r1).is_empty(),
            "a route does not reach itself"
        );
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
