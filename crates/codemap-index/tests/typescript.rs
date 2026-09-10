use codemap_graph::NodeKind;
use codemap_index::{lang::typescript::TypeScriptExtractor, LanguageExtractor};

fn facts() -> codemap_index::FileFacts {
    TypeScriptExtractor.extract(include_str!("fixtures/sample.ts"))
}

#[test]
fn extracts_classes_and_methods() {
    let f = facts();
    let classes: Vec<_> = f
        .containers
        .iter()
        .filter(|s| s.kind == NodeKind::Class)
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(classes, vec!["ChatService"]);
    let m = f
        .symbols
        .iter()
        .find(|s| s.name == "streamResponse")
        .unwrap();
    assert_eq!(m.parent.as_deref(), Some("ChatService"));
}

#[test]
fn extracts_express_route() {
    let f = facts();
    assert_eq!(f.routes.len(), 1);
    assert_eq!(f.routes[0].method, "POST");
    assert_eq!(f.routes[0].path, "/chat/stream");
    assert_eq!(f.routes[0].handler, "chatStream");
}

#[test]
fn this_and_imported_receivers_resolve_and_new_bindings_qualify() {
    let f = facts();
    assert!(
        f.calls
            .iter()
            .find(|c| c.callee == "normalize")
            .unwrap()
            .resolvable
    );
    assert!(
        f.calls
            .iter()
            .find(|c| c.callee == "finish")
            .unwrap()
            .resolvable
    );
    let l = f
        .calls
        .iter()
        .find(|c| c.callee.ends_with("streamResponse"))
        .unwrap();
    assert!(l.resolvable);
    assert_eq!(l.callee, "ChatService.streamResponse");
}

#[test]
fn extensions_cover_js_and_tsx() {
    assert!(TypeScriptExtractor.extensions().contains(&"tsx"));
    assert!(TypeScriptExtractor.extensions().contains(&"js"));
}
