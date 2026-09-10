use codemap_graph::NodeKind;
use codemap_index::{lang::python::PythonExtractor, LanguageExtractor};

fn facts() -> codemap_index::FileFacts {
    let src = include_str!("fixtures/sample.py");
    PythonExtractor.extract(src)
}

#[test]
fn extracts_classes_and_functions() {
    let f = facts();
    let classes: Vec<_> = f
        .containers
        .iter()
        .filter(|s| s.kind == NodeKind::Class)
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(classes, vec!["ChatService"]);

    let mut fns: Vec<_> = f.symbols.iter().map(|s| s.name.as_str()).collect();
    fns.sort();
    assert_eq!(fns, vec!["chat_stream", "finish", "stream_response"]);
}

#[test]
fn methods_record_their_owning_class() {
    let f = facts();
    let m = f
        .symbols
        .iter()
        .find(|s| s.name == "stream_response")
        .unwrap();
    assert_eq!(m.parent.as_deref(), Some("ChatService"));
    let top = f.symbols.iter().find(|s| s.name == "chat_stream").unwrap();
    assert_eq!(top.parent, None);
}

#[test]
fn extracts_fastapi_route_with_method_and_path() {
    let f = facts();
    assert_eq!(f.routes.len(), 1);
    let r = &f.routes[0];
    assert_eq!(r.method, "POST");
    assert_eq!(r.path, "/chat/stream");
    assert_eq!(r.handler, "chat_stream");
}

#[test]
fn classifies_calls_by_resolvability() {
    let f = facts();
    let n = f.calls.iter().find(|c| c.callee == "normalize").unwrap();
    assert!(n.resolvable);
    let s = f.calls.iter().find(|c| c.callee == "finish").unwrap();
    assert!(s.resolvable);
    let l = f
        .calls
        .iter()
        .find(|c| c.callee == "stream_response")
        .unwrap();
    assert!(
        !l.resolvable,
        "local-variable receiver must be marked unresolvable"
    );
}
