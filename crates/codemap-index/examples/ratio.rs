//! Measures the resolvable-call ratio over a directory tree, to compare against
//! the 69.6% pre-heuristic baseline recorded in the spec (section 7).
use codemap_index::{lang::python::PythonExtractor, LanguageExtractor};
use std::path::{Path, PathBuf};

fn py_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if !matches!(
                name.as_ref(),
                "node_modules" | ".git" | "venv" | ".venv" | "__pycache__" | "target"
            ) {
                py_files(&p, out);
            }
        } else if p.extension().map(|x| x == "py").unwrap_or(false) {
            out.push(p);
        }
    }
}

fn main() {
    let root = std::env::args().nth(1).expect("usage: ratio <dir>");
    let mut files = Vec::new();
    py_files(Path::new(&root), &mut files);
    let (mut ok, mut total) = (0usize, 0usize);
    let t0 = std::time::Instant::now();
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        for c in &PythonExtractor.extract(&src).calls {
            total += 1;
            if c.resolvable {
                ok += 1;
            }
        }
    }
    let pct = 100.0 * ok as f64 / total as f64;
    println!("files      {}", files.len());
    println!("elapsed    {:?}", t0.elapsed());
    println!("calls      {total}");
    println!("resolvable {ok}  ({pct:.1}%)");
    println!("baseline   69.6%  (spike, pre-heuristic)");
    println!("delta      {:+.1} pp", pct - 69.6);
}
