use codemap_graph::NodeKind;
use codemap_index::{lang::go::GoExtractor, LanguageExtractor};

fn facts() -> codemap_index::FileFacts {
    GoExtractor.extract(include_str!("fixtures/sample.go"))
}

#[test]
fn methods_attribute_to_receiver_type() {
    let f = facts();
    let types: Vec<_> = f
        .containers
        .iter()
        .filter(|s| s.kind == NodeKind::Class)
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(types, vec!["ChatService"]);
    let m = f.symbols.iter().find(|s| s.name == "Stream").unwrap();
    assert_eq!(
        m.parent.as_deref(),
        Some("ChatService"),
        "receiver type is the owner"
    );
}

#[test]
fn receiver_calls_resolve() {
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
fn detects_handlefunc_route() {
    let f = facts();
    assert_eq!(f.routes.len(), 1);
    assert_eq!(f.routes[0].path, "/chat/stream");
    assert_eq!(
        f.routes[0].method, "ANY",
        "HandleFunc does not specify a method"
    );
    assert_eq!(f.routes[0].handler, "Stream");
}
