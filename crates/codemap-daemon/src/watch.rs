//! Tier 3: the universal filesystem fallback.
//!
//! Works with every agent and with hand-editing, which is what makes the tool
//! agent-agnostic. It cannot attribute a change to an actor or see reads, so
//! Tier 1 hook events supersede it for the same file within a short window.

use crate::event::{ActivityEvent, AgentId, EventKind};
use crate::state::AppState;
use notify::{RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(250);
/// A hook event for a file suppresses filesystem events for it briefly, so one
/// edit is not counted twice with the second attributed to nobody.
pub const HOOK_SUPPRESSION: Duration = Duration::from_millis(1500);

const SKIP: [&str; 7] = [
    "node_modules",
    "target",
    ".git",
    "dist",
    "venv",
    ".venv",
    "__pycache__",
];

fn interesting(p: &Path) -> bool {
    if p.components()
        .any(|c| SKIP.contains(&c.as_os_str().to_string_lossy().as_ref()))
    {
        return false;
    }
    matches!(
        p.extension().and_then(|e| e.to_str()),
        Some("py" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "go" | "rs")
    )
}

/// Blocking watcher; run on a dedicated thread.
pub fn run(root: PathBuf, state: AppState) {
    let (tx, rx) = mpsc::channel();
    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    }) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("codemap: filesystem watcher unavailable: {e}");
            return;
        }
    };
    if let Err(e) = watcher.watch(&root, RecursiveMode::Recursive) {
        eprintln!("codemap: cannot watch {}: {e}", root.display());
        return;
    }

    let mut last: Option<(PathBuf, Instant)> = None;
    while let Ok(Ok(event)) = rx.recv() {
        for path in event.paths.iter().filter(|p| interesting(p)) {
            let now = Instant::now();
            if let Some((p, t)) = &last {
                if p == path && now.duration_since(*t) < DEBOUNCE {
                    continue;
                }
            }
            last = Some((path.clone(), now));

            if state.recently_hooked(path, now) {
                continue; // a Tier 1 adapter already reported this edit
            }
            state.reindex_file(path);
            let ev = ActivityEvent {
                kind: EventKind::Write,
                path: path.clone(),
                range: None,
                agent: AgentId::Unknown,
            };
            if state.ingest(&ev) {
                state.broadcast(&state.graph_payload());
                state.broadcast(&state.focus_payload());
            }
        }
    }
}
