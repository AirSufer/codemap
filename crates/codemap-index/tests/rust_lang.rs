use codemap_graph::NodeKind;
use codemap_index::{lang::rust_lang::RustExtractor, LanguageExtractor};

fn facts() -> codemap_index::FileFacts {
    RustExtractor.extract(include_str!("fixtures/sample_rs.txt"))
}

#[test]
fn impl_blocks_own_their_methods() {
    let f = facts();
    let types: Vec<_> = f
        .containers
        .iter()
        .filter(|s| s.kind == NodeKind::Class)
        .map(|s| s.name.as_str())
        .collect();
    assert!(types.contains(&"ChatService"));
    let m = f.symbols.iter().find(|s| s.name == "stream").unwrap();
    assert_eq!(m.parent.as_deref(), Some("ChatService"));
}

#[test]
fn self_calls_resolve() {
    let f = facts();
    let c = f
        .calls
        .iter()
        .find(|c| c.callee.ends_with("finish"))
        .unwrap();
    assert!(c.resolvable);
    assert_eq!(c.callee, "ChatService.finish");
}

#[test]
fn detects_axum_route() {
    let f = facts();
    assert_eq!(f.routes.len(), 1);
    assert_eq!(f.routes[0].method, "POST");
    assert_eq!(f.routes[0].path, "/chat/stream");
    assert_eq!(f.routes[0].handler, "handler");
}
