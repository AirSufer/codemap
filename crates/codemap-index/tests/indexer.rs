use codemap_graph::NodeKind;
use codemap_index::indexer::index_repo;
use std::path::Path;

fn g() -> codemap_graph::Graph {
    index_repo(Path::new("tests/fixtures/repo"))
}

#[test]
fn skips_vendored_directories() {
    let graph = g();
    assert!(
        !graph
            .nodes()
            .iter()
            .any(|n| n.file.contains("node_modules")),
        "node_modules must never be indexed"
    );
}

#[test]
fn builds_modules_classes_and_functions() {
    let graph = g();
    let kinds = |k: NodeKind| graph.nodes().iter().filter(|n| n.kind == k).count();
    assert_eq!(kinds(NodeKind::Module), 2);
    assert_eq!(kinds(NodeKind::Class), 1);
    assert!(kinds(NodeKind::Function) >= 2);
    assert_eq!(kinds(NodeKind::Route), 1);
}

#[test]
fn route_reaches_the_method_it_calls_across_files() {
    let graph = g();
    let stream = graph
        .nodes()
        .iter()
        .find(|n| n.kind == NodeKind::Function && n.name == "stream")
        .unwrap();
    let routes = graph.routes_reaching(stream.id);
    assert_eq!(
        routes.len(),
        1,
        "POST /chat/stream should reach ChatService.stream"
    );
    assert_eq!(graph.node(routes[0]).name, "POST /chat/stream");
}

#[test]
fn unresolved_calls_are_counted_not_dropped() {
    let graph = g();
    // `mystery.run()` has a parameter receiver with no known type, so it cannot
    // resolve. It must be COUNTED, never silently discarded -- otherwise the graph
    // implies complete call coverage it does not have (spec section 7).
    let caller = graph
        .nodes()
        .iter()
        .find(|n| n.kind == NodeKind::Function && n.name == "chat_stream")
        .unwrap();
    assert!(
        caller.unresolved_calls >= 1,
        "unresolvable call must increment the caller's unresolved_calls"
    );
    assert!(
        !graph.callees(caller.id).is_empty(),
        "resolvable calls must still yield Calls edges"
    );
}
