use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which coding agent produced an event. Tier 3 (filesystem) events have no
/// attributable actor and use `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentId {
    ClaudeCode,
    Codex,
    Gemini,
    Unknown,
}

impl AgentId {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => AgentId::ClaudeCode,
            "codex" => AgentId::Codex,
            "gemini" => AgentId::Gemini,
            _ => AgentId::Unknown,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            AgentId::ClaudeCode => "claude",
            AgentId::Codex => "codex",
            AgentId::Gemini => "gemini",
            AgentId::Unknown => "fs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Read,
    Write,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

/// The single contract every adapter produces and the focus engine consumes.
/// The core never learns which agent is upstream beyond this struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub kind: EventKind,
    pub path: PathBuf,
    pub range: Option<LineRange>,
    pub agent: AgentId,
}
