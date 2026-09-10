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
    // `self.finish()` is qualified with its enclosing class so it cannot
    // collide with other methods named `finish` elsewhere in the repo.
    let s = f
        .calls
        .iter()
        .find(|c| c.callee.ends_with("finish"))
        .unwrap();
    assert!(s.resolvable);
    assert_eq!(s.callee, "ChatService.finish");
    // `svc = ChatService()` then `svc.stream_response()` resolves via the
    // local-binding heuristic and is rewritten to its qualified form.
    let l = f
        .calls
        .iter()
        .find(|c| c.callee.ends_with("stream_response"))
        .unwrap();
    assert!(l.resolvable);
    assert_eq!(l.callee, "ChatService.stream_response");
}

#[test]
fn local_var_bound_to_constructor_resolves_to_class_method() {
    let src = r#"
class ChatService:
    def run(self): pass

def handler():
    svc = ChatService()
    return svc.run()
"#;
    let f = PythonExtractor.extract(src);
    let c = f
        .calls
        .iter()
        .find(|c| c.callee.ends_with("run") && c.from_symbol == "handler")
        .unwrap();
    assert!(c.resolvable, "x = ClassName() then x.run() should resolve");
    assert_eq!(c.callee, "ChatService.run");
}

#[test]
fn local_var_from_unknown_source_stays_unresolvable() {
    let src = r#"
def handler(dep):
    return dep.run()
"#;
    let f = PythonExtractor.extract(src);
    let c = f.calls.iter().find(|c| c.callee == "run").unwrap();
    assert!(!c.resolvable, "parameter receiver has no known type");
}
