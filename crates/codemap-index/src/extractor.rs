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
    /// Parse `src` and return facts. MUST NOT retain the syntax tree —
    /// retaining trees measured 266MB vs 15.9MB on core-service (spec section 17).
    fn extract(&self, src: &str) -> FileFacts;
}
