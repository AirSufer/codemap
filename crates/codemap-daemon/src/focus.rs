//! Holds what the agent is touching now, and a decaying trail of what it
//! touched recently. The trail is what makes the display a record of a session
//! rather than a snapshot.

use crate::event::AgentId;
use codemap_graph::{Graph, NodeId};
use serde::Serialize;
use std::time::{Duration, Instant};

/// Half-life of a trail entry's glow. Spec section 9.
pub const TRAIL_HALF_LIFE: Duration = Duration::from_secs(90);
/// Entries below this weight are dropped entirely.
const CUTOFF: f32 = 0.02;

#[derive(Debug, Clone)]
struct TrailEntry {
    node: NodeId,
    at: Instant,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrailItem {
    pub node: NodeId,
    pub weight: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct FocusSnapshot {
    pub active: Option<NodeId>,
    /// Containment chain root-first, so the UI can light every ancestor.
    pub ancestors: Vec<NodeId>,
    pub trail: Vec<TrailItem>,
    pub following: bool,
    pub agent: &'static str,
}

pub struct FocusEngine {
    active: Option<NodeId>,
    trail: Vec<TrailEntry>,
    following: bool,
    agent: AgentId,
    max_trail: usize,
}

impl Default for FocusEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl FocusEngine {
    pub fn new() -> Self {
        FocusEngine {
            active: None,
            trail: Vec::new(),
            following: true,
            agent: AgentId::Unknown,
            max_trail: 32,
        }
    }

    pub fn touch(&mut self, node: NodeId, agent: AgentId, now: Instant) {
        self.agent = agent;
        self.active = Some(node);
        self.trail.retain(|e| e.node != node);
        self.trail.push(TrailEntry { node, at: now });
        if self.trail.len() > self.max_trail {
            self.trail.remove(0);
        }
    }

    /// The camera follows by default and detaches the moment the human steers.
    pub fn detach(&mut self) {
        self.following = false;
    }
    pub fn attach(&mut self) {
        self.following = true;
    }
    pub fn following(&self) -> bool {
        self.following
    }
    pub fn active(&self) -> Option<NodeId> {
        self.active
    }

    pub fn snapshot(&self, graph: &Graph, now: Instant) -> FocusSnapshot {
        let hl = TRAIL_HALF_LIFE.as_secs_f32();
        let trail: Vec<TrailItem> = self
            .trail
            .iter()
            .rev()
            .map(|e| {
                let age = now.saturating_duration_since(e.at).as_secs_f32();
                TrailItem {
                    node: e.node,
                    weight: 0.5f32.powf(age / hl),
                }
            })
            .filter(|t| t.weight > CUTOFF)
            .collect();
        FocusSnapshot {
            active: self.active,
            ancestors: self.active.map(|a| graph.ancestors(a)).unwrap_or_default(),
            trail,
            following: self.following,
            agent: self.agent.label(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codemap_graph::{EdgeKind, Node, NodeKind};

    fn graph() -> (Graph, NodeId, NodeId) {
        let mut g = Graph::new();
        let m = g.add_node(Node::new(NodeKind::Module, "m", "m.py", 1, 9));
        let f = g.add_node(Node::new(NodeKind::Function, "f", "m.py", 2, 4));
        g.add_edge(m, f, EdgeKind::Contains);
        (g, m, f)
    }

    #[test]
    fn active_node_carries_its_ancestor_chain() {
        let (g, m, f) = graph();
        let mut e = FocusEngine::new();
        let now = Instant::now();
        e.touch(f, AgentId::ClaudeCode, now);
        let s = e.snapshot(&g, now);
        assert_eq!(s.active, Some(f));
        assert_eq!(s.ancestors, vec![m]);
        assert_eq!(s.agent, "claude");
    }

    #[test]
    fn trail_decays_by_half_every_half_life() {
        let (g, m, f) = graph();
        let mut e = FocusEngine::new();
        let t0 = Instant::now();
        e.touch(m, AgentId::Codex, t0);
        e.touch(f, AgentId::Codex, t0 + TRAIL_HALF_LIFE);
        let s = e.snapshot(&g, t0 + TRAIL_HALF_LIFE);
        // newest first
        assert_eq!(s.trail[0].node, f);
        assert!((s.trail[0].weight - 1.0).abs() < 0.01);
        assert!(
            (s.trail[1].weight - 0.5).abs() < 0.01,
            "one half-life -> 0.5"
        );
    }

    #[test]
    fn stale_trail_entries_are_dropped() {
        let (g, m, _f) = graph();
        let mut e = FocusEngine::new();
        let t0 = Instant::now();
        e.touch(m, AgentId::Gemini, t0);
        let s = e.snapshot(&g, t0 + TRAIL_HALF_LIFE * 8);
        assert!(s.trail.is_empty(), "8 half-lives is below the cutoff");
    }

    #[test]
    fn retouching_a_node_moves_it_to_the_front_without_duplicating() {
        let (g, m, f) = graph();
        let mut e = FocusEngine::new();
        let t0 = Instant::now();
        e.touch(m, AgentId::ClaudeCode, t0);
        e.touch(f, AgentId::ClaudeCode, t0);
        e.touch(m, AgentId::ClaudeCode, t0);
        let s = e.snapshot(&g, t0);
        assert_eq!(s.trail.len(), 2);
        assert_eq!(s.trail[0].node, m);
    }

    #[test]
    fn following_detaches_and_reattaches() {
        let mut e = FocusEngine::new();
        assert!(e.following(), "follow is the default");
        e.detach();
        assert!(!e.following());
        e.attach();
        assert!(e.following());
    }
}
