use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

const SKIP: [&str; 7] = [
    "node_modules",
    "target",
    ".git",
    "dist",
    "venv",
    ".venv",
    "__pycache__",
];

pub fn walk_repo(root: &Path) -> Vec<PathBuf> {
    WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|e| !SKIP.contains(&e.file_name().to_string_lossy().as_ref()))
        .build()
        .flatten()
        .map(|e| e.into_path())
        .filter(|p| p.is_file())
        .collect()
}
