//! The daemon: one per repo, started explicitly, never on its own.

use crate::adapter::normalize;
use crate::event::AgentId;
use crate::state::{socket_path, AppState, ServerMsg};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{TcpListener, UnixListener};

const CLIENT_HTML: &str = include_str!("../client/index.html");

pub struct Daemon {
    pub root: PathBuf,
    pub port: u16,
}

impl Daemon {
    pub async fn run(root: PathBuf, port: u16) -> std::io::Result<()> {
        let state = AppState::new(&root);
        let sock = socket_path(&root);
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock)?;

        // Tier 3 watcher on its own thread; notify's API is blocking.
        {
            let st = state.clone();
            let r = root.clone();
            std::thread::spawn(move || crate::watch::run(r, st));
        }

        // Tier 1 ingest: hooks connect, write one JSON line, and exit.
        {
            let st = state.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        continue;
                    };
                    let st = st.clone();
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stream).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            handle_hook_line(&st, &line);
                        }
                    });
                }
            });
        }

        let app = Router::new()
            .route("/", get(|| async { Html(CLIENT_HTML) }))
            .route("/ws", get(ws_handler))
            .route("/api/focus", get(focus_json))
            .with_state(state.clone());

        let addr = format!("127.0.0.1:{port}");
        let tcp = TcpListener::bind(&addr).await?;
        let actual = tcp.local_addr()?.port();
        println!("  open      http://127.0.0.1:{actual}");
        println!("  explore   codemap explore {}", root.display());
        println!("  ticker    codemap ticker {}", root.display());
        // Record the live port so `codemap ticker` and `install-hooks` can find us.
        let _ = std::fs::write(sock.with_extension("port"), actual.to_string());

        axum::serve(tcp, app).await?;
        let _ = std::fs::remove_file(&sock);
        Ok(())
    }
}

fn handle_hook_line(state: &AppState, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    // Control messages from the ticker / browser share the socket.
    if let Some(cmd) = v.get("codemap_cmd").and_then(Value::as_str) {
        match cmd {
            "follow" => state.set_following(true),
            "detach" => state.set_following(false),
            _ => {}
        }
        state.broadcast(&state.focus_payload());
        return;
    }
    let agent = v
        .get("codemap_agent")
        .and_then(Value::as_str)
        .map(AgentId::parse)
        .unwrap_or(AgentId::Unknown);
    if let Some(ev) = normalize(agent, &v) {
        if state.ingest(&ev) {
            state.broadcast(&state.focus_payload());
        }
    }
}

/// Where the agent is, as JSON. Backs `codemap agent`, so the CLI does not
/// need to speak WebSocket for a single fact.
async fn focus_json(State(state): State<AppState>) -> impl IntoResponse {
    use std::time::Instant;
    let g = state.inner.lock().unwrap();
    let snap = g.focus.snapshot(&g.graph, Instant::now());
    let body = match snap.active {
        Some(id) => {
            let n = g.graph.node(id);
            serde_json::json!({
                "name": n.name,
                "qualified_name": n.qualified_name,
                "file": n.file,
                "line": n.line_start,
                "kind": format!("{:?}", n.kind).to_lowercase(),
                "agent": snap.agent,
                "following": snap.following,
                "ago_secs": snap.last_event_secs.unwrap_or(0),
            })
        }
        None => serde_json::json!({ "name": "", "agent": snap.agent }),
    };
    ([("content-type", "application/json")], body.to_string())
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| client_loop(socket, state))
}

async fn client_loop(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let mut rx = state.tx.subscribe();

    // Send the full graph and current focus on connect.
    for msg in [state.graph_payload(), state.focus_payload()] {
        if let Ok(s) = serde_json::to_string(&msg) {
            if sink.send(Message::Text(s.into())).await.is_err() {
                return;
            }
        }
    }

    let st = state.clone();
    let mut send_task = tokio::spawn(async move {
        while let Ok(s) = rx.recv().await {
            if sink.send(Message::Text(s.into())).await.is_err() {
                break;
            }
        }
        drop(sink);
    });

    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            let Message::Text(t) = msg else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&t) else {
                continue;
            };
            match v.get("type").and_then(Value::as_str) {
                Some("detail") => {
                    if let Some(id) = v.get("id").and_then(Value::as_u64) {
                        // The client measures its own panel, so the tree is
                        // rendered to the width it will actually be shown at.
                        let w = v
                            .get("width")
                            .and_then(Value::as_u64)
                            .unwrap_or(72)
                            .clamp(36, 110) as usize;
                        if let Some(d) = st.detail(id as u32, w) {
                            st.broadcast(&d);
                        }
                    }
                }
                Some("follow") => {
                    let on = v.get("on").and_then(Value::as_bool).unwrap_or(true);
                    st.set_following(on);
                    st.broadcast(&st.focus_payload());
                }
                Some("send_to_agent") => {
                    let prompt = v.get("prompt").and_then(Value::as_str).unwrap_or("");
                    crate::server::deliver_prompt(prompt);
                }
                Some("open_editor") => {
                    let file = v.get("file").and_then(Value::as_str).unwrap_or("");
                    let line = v.get("line").and_then(Value::as_u64).unwrap_or(1);
                    open_in_editor(file, line as u32);
                }
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }
}

/// Types the prompt into the agent's tmux pane WITHOUT pressing Enter, falling
/// back to the clipboard everywhere else. Never auto-submits: injecting a prompt
/// mid-turn while an agent is working would be destructive (spec section 13).
pub fn deliver_prompt(prompt: &str) {
    if let Some(pane) = find_agent_pane() {
        let ok = std::process::Command::new("tmux")
            .args(["send-keys", "-t", &pane, prompt])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return;
        }
    }
    copy_to_clipboard(prompt);
}

const AGENT_NAMES: [&str; 3] = ["claude", "codex", "gemini"];

/// Finds the tmux pane running a coding agent.
///
/// `pane_current_command` is unreliable: Claude Code reports its version string
/// (e.g. "2.1.263") rather than "claude", so matching on it misses real panes.
/// Inspecting the processes attached to each pane's tty is what actually works.
pub fn find_agent_pane() -> Option<String> {
    if let Ok(p) = std::env::var("CODEMAP_AGENT_PANE") {
        return Some(p);
    }
    let out = std::process::Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{pane_id} #{pane_tty} #{pane_current_command}",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut fallback = None;
    for line in text.lines() {
        let mut parts = line.split(' ');
        let (Some(id), Some(tty)) = (parts.next(), parts.next()) else {
            continue;
        };
        let cmd = parts.next().unwrap_or("");
        if AGENT_NAMES.contains(&cmd) {
            return Some(id.to_string());
        }
        if fallback.is_none() && pane_runs_agent(tty) {
            fallback = Some(id.to_string());
        }
    }
    fallback
}

fn pane_runs_agent(tty: &str) -> bool {
    let short = tty.strip_prefix("/dev/").unwrap_or(tty);
    let Ok(out) = std::process::Command::new("ps")
        .args(["-t", short, "-o", "command="])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout).lines().any(|l| {
        let first = l.split_whitespace().next().unwrap_or("");
        let base = first.rsplit('/').next().unwrap_or(first);
        AGENT_NAMES.contains(&base)
    })
}

pub fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    let cmd = if cfg!(target_os = "macos") {
        "pbcopy"
    } else {
        "xclip"
    };
    if let Ok(mut child) = std::process::Command::new(cmd)
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

fn open_in_editor(file: &str, line: u32) {
    let tmpl = std::env::var("CODEMAP_OPEN_COMMAND").unwrap_or_default();
    if !tmpl.is_empty() {
        let cmd = tmpl
            .replace("{file}", file)
            .replace("{line}", &line.to_string());
        let _ = std::process::Command::new("sh").arg("-c").arg(cmd).status();
        return;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    let _ = std::process::Command::new(&editor)
        .arg(format!("+{line}"))
        .arg(file)
        .status();
}

/// Fire-and-forget client used by the hook binary. Must be fast: the agent
/// blocks on its hooks, so a missing socket exits immediately.
pub fn send_to_daemon(root: &Path, payload: &str) -> bool {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    let sock = socket_path(root);
    if !sock.exists() {
        return false;
    }
    match UnixStream::connect(&sock) {
        Ok(mut s) => {
            let _ = s.write_all(payload.as_bytes());
            let _ = s.write_all(b"\n");
            true
        }
        Err(_) => false,
    }
}

pub fn daemon_port(root: &Path) -> Option<u16> {
    std::fs::read_to_string(socket_path(root).with_extension("port"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// A socket file surviving a crashed daemon must not look like a live one, so
/// this actually connects. On a unix socket with no listener that fails
/// immediately with ECONNREFUSED, so the hook fast path stays cheap.
pub fn is_running(root: &Path) -> bool {
    use std::os::unix::net::UnixStream;
    let sock = socket_path(root);
    if !sock.exists() {
        return false;
    }
    match UnixStream::connect(&sock) {
        Ok(_) => true,
        Err(_) => {
            // Stale: clean it up so the next `start` is not blocked forever.
            let _ = std::fs::remove_file(&sock);
            let _ = std::fs::remove_file(sock.with_extension("port"));
            false
        }
    }
}

impl ServerMsg {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}
