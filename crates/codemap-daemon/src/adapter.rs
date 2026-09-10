//! Normalizes each agent's hook payload into an `ActivityEvent`.
//!
//! Verified 2026-09-10 against the three agents' hook docs: all deliver JSON on
//! stdin carrying a tool name and the model's raw tool arguments. The shapes
//! differ enough to need one normalizer each, but nothing downstream changes.

use crate::event::{ActivityEvent, AgentId, EventKind, LineRange};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Parse a hook payload. `agent` comes from the hook command's own `--agent`
/// flag rather than the payload, since the payloads do not identify themselves.
pub fn normalize(agent: AgentId, payload: &Value) -> Option<ActivityEvent> {
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let input = payload.get("tool_input")?;
    match agent {
        AgentId::ClaudeCode => claude(tool, input),
        AgentId::Gemini => gemini(tool, input),
        AgentId::Codex => codex(tool, input),
        AgentId::Unknown => None,
    }
    .map(|(kind, path, range)| ActivityEvent {
        kind,
        path,
        range,
        agent,
    })
}

type Norm = (EventKind, PathBuf, Option<LineRange>);

fn str_field(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .map(str::to_string)
}

/// Claude Code: Read/Edit/Write/MultiEdit/Grep/Glob, `file_path` plus either
/// `offset`+`limit` (reads) or `old_string` (edits).
fn claude(tool: &str, input: &Value) -> Option<Norm> {
    let path = str_field(input, &["file_path", "path", "notebook_path"]);
    match tool {
        "Read" | "NotebookRead" => {
            let p = PathBuf::from(path?);
            let range = match (
                input.get("offset").and_then(Value::as_u64),
                input.get("limit").and_then(Value::as_u64),
            ) {
                (Some(o), Some(l)) => Some(LineRange {
                    start: o as u32,
                    end: (o + l) as u32,
                }),
                _ => None,
            };
            Some((EventKind::Read, p, range))
        }
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let p = PathBuf::from(path?);
            // Edits carry `old_string`, not line numbers. Locating it in the
            // file is what turns "edited server.py" into "edited
            // ChatService.stream" (spec section 8).
            let range = str_field(input, &["old_string"]).and_then(|needle| locate(&p, &needle));
            Some((EventKind::Write, p, range))
        }
        "Grep" | "Glob" => Some((EventKind::Search, PathBuf::from(path?), None)),
        _ => None,
    }
}

/// Gemini CLI: snake_case tool names, `absolute_path` on reads.
fn gemini(tool: &str, input: &Value) -> Option<Norm> {
    let path = str_field(input, &["absolute_path", "file_path", "path"]);
    match tool {
        "read_file" | "read_many_files" => {
            let p = PathBuf::from(path?);
            let range = match (
                input.get("offset").and_then(Value::as_u64),
                input.get("limit").and_then(Value::as_u64),
            ) {
                (Some(o), Some(l)) => Some(LineRange {
                    start: o as u32,
                    end: (o + l) as u32,
                }),
                _ => None,
            };
            Some((EventKind::Read, p, range))
        }
        "write_file" | "replace" | "edit" => Some((EventKind::Write, PathBuf::from(path?), None)),
        "search_file_content" | "glob" | "list_directory" => {
            Some((EventKind::Search, PathBuf::from(path?), None))
        }
        _ => None,
    }
}

/// Codex CLI: file edits arrive as a patch inside `tool_input.command` rather
/// than as structured arguments, so the path is recovered from the patch header.
fn codex(tool: &str, input: &Value) -> Option<Norm> {
    match tool {
        "apply_patch" | "ApplyPatch" => {
            let cmd = str_field(input, &["command", "patch", "input"])?;
            let (path, range) = parse_patch(&cmd)?;
            Some((EventKind::Write, path, range))
        }
        "Read" | "read_file" => Some((
            EventKind::Read,
            PathBuf::from(str_field(input, &["path", "file_path"])?),
            None,
        )),
        "Bash" | "shell" | "local_shell" => None,
        _ => None,
    }
}

/// Finds `needle` in the file and returns the 1-based line range it spans.
/// Returns None when the file is unreadable or the text is absent or ambiguous.
pub fn locate(path: &Path, needle: &str) -> Option<LineRange> {
    if needle.trim().is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let idx = text.find(needle)?;
    // Ambiguous matches would point at the wrong symbol; prefer no range.
    if text[idx + 1..].contains(needle) {
        return None;
    }
    // Count newlines, not lines: a prefix ending mid-line would
    // otherwise count that partial line and shift the result by one.
    let start = text[..idx].matches('\n').count() as u32 + 1;
    let span = needle.lines().count().max(1) as u32;
    Some(LineRange {
        start,
        end: start + span - 1,
    })
}

/// Recovers a file path, and a line range when a unified-diff hunk header is
/// present, from an apply_patch body.
pub fn parse_patch(patch: &str) -> Option<(PathBuf, Option<LineRange>)> {
    let mut path: Option<PathBuf> = None;
    let mut range: Option<LineRange> = None;
    for line in patch.lines() {
        let t = line.trim();
        for marker in ["*** Update File:", "*** Add File:", "*** Delete File:"] {
            if let Some(rest) = t.strip_prefix(marker) {
                path = Some(PathBuf::from(rest.trim()));
            }
        }
        if path.is_none() {
            if let Some(rest) = t.strip_prefix("+++ ") {
                let r = rest.trim().trim_start_matches("b/");
                if r != "/dev/null" {
                    path = Some(PathBuf::from(r));
                }
            }
        }
        // @@ -12,7 +12,9 @@
        if range.is_none() && t.starts_with("@@") {
            if let Some(plus) = t.split('+').nth(1) {
                let nums = plus.split(' ').next().unwrap_or("");
                let mut it = nums.split(',');
                if let Some(start) = it.next().and_then(|x| x.parse::<u32>().ok()) {
                    let len = it.next().and_then(|x| x.parse::<u32>().ok()).unwrap_or(1);
                    range = Some(LineRange {
                        start,
                        end: start + len,
                    });
                }
            }
        }
    }
    path.map(|p| (p, range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_read_carries_offset_and_limit() {
        let e = normalize(
            AgentId::ClaudeCode,
            &json!({"tool_name":"Read","tool_input":{"file_path":"/a/b.py","offset":10,"limit":40}}),
        )
        .unwrap();
        assert_eq!(e.kind, EventKind::Read);
        assert_eq!(e.path, PathBuf::from("/a/b.py"));
        assert_eq!(e.range.unwrap().start, 10);
        assert_eq!(e.range.unwrap().end, 50);
    }

    #[test]
    fn claude_edit_is_a_write_without_a_range() {
        let e = normalize(
            AgentId::ClaudeCode,
            &json!({"tool_name":"Edit","tool_input":{"file_path":"/a/b.py","old_string":"x","new_string":"y"}}),
        )
        .unwrap();
        assert_eq!(e.kind, EventKind::Write);
        assert!(e.range.is_none());
    }

    #[test]
    fn gemini_uses_absolute_path_and_snake_case_tools() {
        let e = normalize(
            AgentId::Gemini,
            &json!({"tool_name":"read_file","tool_input":{"absolute_path":"/x/y.ts","offset":5,"limit":5}}),
        )
        .unwrap();
        assert_eq!(e.path, PathBuf::from("/x/y.ts"));
        assert_eq!(e.range.unwrap().end, 10);

        let w = normalize(
            AgentId::Gemini,
            &json!({"tool_name":"replace","tool_input":{"file_path":"/x/y.ts"}}),
        )
        .unwrap();
        assert_eq!(w.kind, EventKind::Write);
    }

    #[test]
    fn codex_recovers_path_and_range_from_a_patch() {
        let patch = "*** Begin Patch\n*** Update File: src/chat.py\n@@ -12,7 +12,9 @@\n-old\n+new\n*** End Patch";
        let e = normalize(
            AgentId::Codex,
            &json!({"tool_name":"apply_patch","tool_input":{"command":patch}}),
        )
        .unwrap();
        assert_eq!(e.kind, EventKind::Write);
        assert_eq!(e.path, PathBuf::from("src/chat.py"));
        let r = e.range.unwrap();
        assert_eq!((r.start, r.end), (12, 21));
    }

    #[test]
    fn codex_also_reads_unified_diff_headers() {
        let patch = "--- a/src/x.go\n+++ b/src/x.go\n@@ -3,2 +4,5 @@\n";
        let (p, r) = parse_patch(patch).unwrap();
        assert_eq!(p, PathBuf::from("src/x.go"));
        assert_eq!(r.unwrap().start, 4);
    }

    #[test]
    fn edit_locates_old_string_to_a_line_range() {
        let dir = std::env::temp_dir().join("codemap-adapter-test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.py");
        std::fs::write(
            &f,
            "import os\n\nclass S:\n    def run(self):\n        return UNIQUE_MARKER\n",
        )
        .unwrap();
        let e = normalize(
            AgentId::ClaudeCode,
            &json!({"tool_name":"Edit","tool_input":{
                "file_path": f.to_str().unwrap(),
                "old_string":"return UNIQUE_MARKER","new_string":"return 1"}}),
        )
        .unwrap();
        let r = e.range.expect("old_string should yield a range");
        assert_eq!(r.start, 5, "UNIQUE_MARKER lives on line 5");
    }

    #[test]
    fn ambiguous_old_string_yields_no_range_rather_than_a_wrong_one() {
        let dir = std::env::temp_dir().join("codemap-adapter-test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("dup.py");
        std::fs::write(&f, "x = 1\ny = 2\nx = 1\n").unwrap();
        assert!(locate(&f, "x = 1").is_none(), "two matches must not guess");
    }

    #[test]
    fn unknown_tools_are_ignored_rather_than_guessed() {
        assert!(normalize(
            AgentId::ClaudeCode,
            &json!({"tool_name":"WebFetch","tool_input":{"url":"http://x"}})
        )
        .is_none());
    }
}
