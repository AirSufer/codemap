use std::process::Command;

fn run(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_codemap"))
        .args(args)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

const REPO: &str = "../codemap-index/tests/fixtures/repo";

#[test]
fn index_prints_a_summary() {
    let s = run(&["index", REPO]);
    assert!(s.contains("modules"), "got: {s}");
    assert!(s.contains("routes"), "got: {s}");
    assert!(
        s.contains("resolved"),
        "must report call-resolution coverage"
    );
}

#[test]
fn index_json_is_parseable_and_has_nodes_and_edges() {
    let s = run(&["index", REPO, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
    assert!(v["nodes"].as_array().unwrap().len() >= 5);
    assert!(v["edges"].as_array().is_some());
}

#[test]
fn reach_names_the_route_that_reaches_a_symbol() {
    let s = run(&["reach", REPO, "stream"]);
    assert!(s.contains("POST /chat/stream"), "got: {s}");
}

#[test]
fn reach_on_unknown_symbol_exits_cleanly() {
    let out = Command::new(env!("CARGO_BIN_EXE_codemap"))
        .args(["reach", REPO, "nope"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found"));
}
