use crate::node::{EdgeKind, Node, NodeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    /// Call-site position for Calls edges; (0,0) for structural edges. Needed
    /// so `--vimgrep` points at the call, not the enclosing function.
    #[serde(default)]
    pub line: u32,
    #[serde(default)]
    pub col: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, mut n: Node) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        n.id = id;
        self.nodes.push(n);
        id
    }

    pub fn add_edge(&mut self, from: NodeId, to: NodeId, kind: EdgeKind) {
        self.edges.push(Edge {
            from,
            to,
            kind,
            line: 0,
            col: 0,
        });
    }

    /// Same, but records where the call is written.
    pub fn add_call_edge(&mut self, from: NodeId, to: NodeId, line: u32, col: u32) {
        self.edges.push(Edge {
            from,
            to,
            kind: EdgeKind::Calls,
            line,
            col,
        });
    }

    /// Calls edges out of `id`, with their call-site positions.
    pub fn call_sites(&self, id: NodeId) -> Vec<&Edge> {
        self.edges
            .iter()
            .filter(|e| e.from == id && e.kind == EdgeKind::Calls)
            .collect()
    }

    /// Calls edges into `id`.
    pub fn call_sites_into(&self, id: NodeId) -> Vec<&Edge> {
        self.edges
            .iter()
            .filter(|e| e.to == id && e.kind == EdgeKind::Calls)
            .collect()
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }
    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.0 as usize]
    }
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn out(&self, id: NodeId, kind: EdgeKind) -> Vec<NodeId> {
        self.edges
            .iter()
            .filter(|e| e.from == id && e.kind == kind)
            .map(|e| e.to)
            .collect()
    }

    fn inc(&self, id: NodeId, kind: EdgeKind) -> Vec<NodeId> {
        self.edges
            .iter()
            .filter(|e| e.to == id && e.kind == kind)
            .map(|e| e.from)
            .collect()
    }

    pub fn children(&self, id: NodeId) -> Vec<NodeId> {
        self.out(id, EdgeKind::Contains)
    }
    pub fn parents_of(&self, id: NodeId) -> Vec<NodeId> {
        self.inc(id, EdgeKind::Contains)
    }
    pub fn callees(&self, id: NodeId) -> Vec<NodeId> {
        self.out(id, EdgeKind::Calls)
    }
    pub fn callers(&self, id: NodeId) -> Vec<NodeId> {
        self.inc(id, EdgeKind::Calls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeKind;

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
