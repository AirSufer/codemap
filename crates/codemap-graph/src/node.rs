use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Package,
    Module,
    Class,
    Function,
    Route,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    Contains,
    Calls,
    Imports,
}

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
    /// Callee names that resolved to nothing in this repo — stdlib and
    /// third-party. Shown so "unresolved" is not confused with "external".
    #[serde(default)]
    pub dependencies: Vec<String>,
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
            dependencies: Vec::new(),
        }
    }
}
