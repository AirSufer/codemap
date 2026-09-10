use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

/// Never useful in a code map: dependency trees, build output, VCS internals.
const VENDOR: [&str; 14] = [
    "node_modules",
    "target",
    ".git",
    "dist",
    "build",
    "venv",
    ".venv",
    "site-packages",
    "__pycache__",
    "vendor",
    "third_party",
    ".next",
    "coverage",
    ".tox",
];

/// Real code, but not what you are navigating when you ask "where does this
/// work land?". Excluded by default; `--all` brings them back.
const NOISE: [&str; 13] = [
    "tests",
    "test",
    "__tests__",
    "spec",
    "e2e",
    "fixtures",
    "testdata",
    "migrations",
    "alembic",
    "examples",
    "docs",
    "stories",
    "scripts",
];

/// Machine-written files. Indexing them buries hand-written code.
fn is_generated(p: &Path) -> bool {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    name.ends_with("_pb2.py")
        || name.ends_with("_pb2_grpc.py")
        || name.ends_with(".pb.go")
        || name.ends_with("_grpc.pb.go")
        || name.ends_with(".d.ts")
        || name.ends_with(".min.js")
        || name.contains(".generated.")
        || name.ends_with("_generated.go")
        || p.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == "baml_client" || s == "__generated__" || s == "generated" || s == "gen"
        })
}

/// Test files that live outside any test directory, matched by the naming
/// conventions each ecosystem actually uses.
fn is_test_file(p: &Path) -> bool {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    name == "conftest.py"
        || name.starts_with("test_")
        || name.ends_with("_test.py")
        || name.ends_with("_test.go")
        || name.ends_with("_test.rs")
        || name.ends_with(".test.ts")
        || name.ends_with(".test.tsx")
        || name.ends_with(".test.js")
        || name.ends_with(".spec.ts")
        || name.ends_with(".spec.js")
}

fn excluded(name: &str, include_all: bool) -> bool {
    if VENDOR.contains(&name) {
        return true;
    }
    !include_all && NOISE.contains(&name.to_ascii_lowercase().as_str())
}

pub fn walk_repo(root: &Path) -> Vec<PathBuf> {
    walk_repo_opts(root, false)
}

/// `include_all` keeps tests, migrations, examples and docs in the graph.
pub fn walk_repo_opts(root: &Path, include_all: bool) -> Vec<PathBuf> {
    WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(move |e| !excluded(&e.file_name().to_string_lossy(), include_all))
        .build()
        .flatten()
        .map(|e| e.into_path())
        .filter(|p| p.is_file() && !is_generated(p))
        .filter(|p| include_all || !is_test_file(p))
        .collect()
}
