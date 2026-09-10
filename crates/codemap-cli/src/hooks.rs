//! Writes hook configuration for whichever agents are installed.
//!
//! Each agent's config lives in a different place and uses different event
//! names, but all three run a command with JSON on stdin, so one hook binary
//! serves all three (verified 2026-09-10, spec section 6.1).

use serde_json::{json, Value};
use std::path::PathBuf;

pub struct Target {
    pub name: &'static str,
    pub config: PathBuf,
    pub present: bool,
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

pub fn targets() -> Vec<Target> {
    let h = home();
    vec![
        Target {
            name: "claude",
            config: h.join(".claude/settings.json"),
            present: h.join(".claude").is_dir(),
        },
        Target {
            name: "codex",
            config: h.join(".codex/hooks.json"),
            present: h.join(".codex").is_dir(),
        },
        Target {
            name: "gemini",
            config: h.join(".gemini/settings.json"),
            present: h.join(".gemini").is_dir(),
        },
    ]
}

fn hook_cmd(agent: &str) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "codemap".into());
    format!("{exe} hook --agent {agent}")
}

fn read_json(p: &PathBuf) -> Value {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

/// Returns the config text that WOULD be written, without writing it.
/// Codex records trust against a hook definition's hash, so its exact text has
/// to be shown for review before it will run (spec section 15).
pub fn preview(agent: &str) -> Value {
    let cmd = hook_cmd(agent);
    match agent {
        "claude" => json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": "Read|Edit|Write|MultiEdit|Grep|Glob|NotebookRead|NotebookEdit",
                    "hooks": [{"type":"command","command":cmd,"timeout":5}]
                }]
            }
        }),
        "codex" => json!({
            "hooks": {
                "PostToolUse": [{"command": cmd}]
            }
        }),
        "gemini" => json!({
            "hooks": {
                "AfterTool": [{
                    "matcher": "read_file|write_file|replace|search_file_content|glob|read_many_files",
                    "hooks": [{"type":"command","command":cmd,"timeout":5}]
                }]
            }
        }),
        _ => json!({}),
    }
}

/// Merge our hook into the agent's existing config without disturbing others.
pub fn install(agent: &str) -> std::io::Result<PathBuf> {
    let t = targets()
        .into_iter()
        .find(|t| t.name == agent)
        .ok_or_else(|| std::io::Error::other("unknown agent"))?;
    if let Some(dir) = t.config.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut cfg = read_json(&t.config);
    let add = preview(agent);
    let event_key = match agent {
        "gemini" => "AfterTool",
        _ => "PostToolUse",
    };
    let ours = add["hooks"][event_key].clone();
    let hooks = cfg
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("config is not an object"))?
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let arr = hooks
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("hooks is not an object"))?
        .entry(event_key)
        .or_insert_with(|| json!([]));
    let list = arr
        .as_array_mut()
        .ok_or_else(|| std::io::Error::other("event is not an array"))?;
    // Idempotent: drop any previous codemap entry before adding ours.
    list.retain(|e| {
        !serde_json::to_string(e)
            .unwrap_or_default()
            .contains("codemap")
    });
    for item in ours.as_array().cloned().unwrap_or_default() {
        list.push(item);
    }
    std::fs::write(&t.config, serde_json::to_string_pretty(&cfg)?)?;
    Ok(t.config)
}

pub fn uninstall(agent: &str) -> std::io::Result<()> {
    let Some(t) = targets().into_iter().find(|t| t.name == agent) else {
        return Ok(());
    };
    if !t.config.exists() {
        return Ok(());
    }
    let mut cfg = read_json(&t.config);
    if let Some(hooks) = cfg.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (_k, v) in hooks.iter_mut() {
            if let Some(list) = v.as_array_mut() {
                list.retain(|e| {
                    !serde_json::to_string(e)
                        .unwrap_or_default()
                        .contains("codemap")
                });
            }
        }
    }
    std::fs::write(&t.config, serde_json::to_string_pretty(&cfg)?)?;
    Ok(())
}
