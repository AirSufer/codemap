//! The optional tmux ticker. One of several sinks, never an assumed surface:
//! plenty of users live in VS Code terminals, iTerm or Warp (spec section 12).

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default, Clone)]
struct View {
    repo: String,
    unresolved: u32,
    resolved: u32,
    last_event: Option<u64>,
    crumb: String,
    active: String,
    loc: String,
    callers: Vec<String>,
    callees: Vec<String>,
    routes: Vec<String>,
    trail: Vec<String>,
    following: bool,
    agent: String,
    connected: bool,
}

pub fn run(port: u16, repo: String) -> std::io::Result<()> {
    let view = Arc::new(Mutex::new(View {
        repo,
        ..View::default()
    }));
    {
        let v = view.clone();
        std::thread::spawn(move || poll_loop(port, v));
    }

    enable_raw_mode()?;
    let mut out = std::io::stdout();
    out.execute(EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    loop {
        let v = view.lock().unwrap().clone();
        term.draw(|f| render(f, &v))?;
        if event::poll(Duration::from_millis(400))? {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('f') => toggle_follow(port),
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    std::io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

fn render(f: &mut Frame, v: &View) {
    let dim = Style::new().fg(Color::Rgb(123, 131, 151));
    let dimmer = Style::new().fg(Color::Rgb(86, 94, 115));
    let agent = Style::new().fg(Color::Rgb(242, 180, 90));
    let route = Style::new().fg(Color::Rgb(111, 211, 160));
    let warn = Style::new().fg(Color::Rgb(238, 138, 99));
    let text = Style::new().fg(Color::Rgb(223, 227, 236));

    // Labels dim, values bright, caveats as sentences.
    let field = |label: &str, items: &Vec<String>, style: Style, empty: &str| {
        let val = if items.is_empty() {
            Span::styled(empty.to_string(), dimmer)
        } else {
            Span::styled(
                items.iter().take(4).cloned().collect::<Vec<_>>().join("  "),
                style,
            )
        };
        Line::from(vec![Span::styled(format!("{label:<10}"), dimmer), val])
    };

    if !v.connected {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("not running", dimmer)),
                Line::from(""),
                Line::from(vec![
                    Span::styled("No daemon for ", dim),
                    Span::styled(v.repo.clone(), text),
                ]),
                Line::from(Span::styled(
                    "start one in another pane; this will attach on its own",
                    dimmer,
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled("$ ", dimmer),
                    Span::styled("codemap start .", route),
                ]),
            ])
            .block(shell(" codemap ")),
            f.area(),
        );
        return;
    }

    let mut body = vec![Line::from(vec![
        Span::styled(v.crumb.clone(), dim),
        Span::styled(if v.crumb.is_empty() { "" } else { "  " }, dim),
    ])];

    if v.active.is_empty() {
        body.push(Line::from(Span::styled("waiting for your agent…", dimmer)));
    } else {
        body.push(Line::from(vec![
            Span::styled("● ", agent),
            Span::styled(v.active.clone(), agent),
            Span::styled(format!("   {}", v.loc), dimmer),
        ]));
        if v.unresolved > 0 {
            let total = v.unresolved + v.resolved;
            body.push(Line::from(Span::styled(
                format!(
                    "           {} of {} call sites unresolved",
                    v.unresolved, total
                ),
                warn,
            )));
        }
    }
    body.push(Line::from(""));
    body.push(field("called by", &v.callers, text, "—"));
    body.push(field("calls", &v.callees, text, "—"));
    body.push(if v.routes.is_empty() {
        Line::from(vec![
            Span::styled(format!("{:<10}", "routes"), dimmer),
            Span::styled("none found", dimmer),
            Span::styled("  — unknown, not unreachable", dimmer),
        ])
    } else {
        field("routes", &v.routes, route, "—")
    });
    body.push(Line::from(vec![
        Span::styled(format!("{:<10}", "agent"), dimmer),
        Span::styled(v.agent.clone(), text),
        Span::styled(
            match v.last_event {
                Some(a) if a < 2 => "  · just now".to_string(),
                Some(a) if a < 60 => format!("  · {a}s ago"),
                Some(a) => format!("  · {}m ago", a / 60),
                None => String::new(),
            },
            dimmer,
        ),
    ]));
    body.push(field("trail", &v.trail, dim, "—"));
    body.push(Line::from(if v.following {
        Span::styled(
            "f follow · q quit                       90s half-life",
            dimmer,
        )
    } else {
        Span::styled("detached (you steered) · f re-attach · q quit", dimmer)
    }));

    f.render_widget(Paragraph::new(body).block(shell(" codemap ")), f.area());
}

fn shell(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Rgb(37, 42, 58)))
        .title(title)
}

fn toggle_follow(port: u16) {
    let _ = std::process::Command::new("curl")
        .args(["-s", &format!("http://127.0.0.1:{port}/toggle-follow")])
        .output();
}

/// Reads the daemon's WebSocket feed. Implemented with a raw handshake so the
/// ticker adds no dependency the daemon does not already carry.
fn poll_loop(port: u16, view: Arc<Mutex<View>>) {
    loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => {
                if let Err(_e) = pump(stream, &view) {
                    view.lock().unwrap().connected = false;
                }
            }
            Err(_) => {
                view.lock().unwrap().connected = false;
            }
        }
        std::thread::sleep(Duration::from_millis(900));
    }
}

fn pump(mut stream: std::net::TcpStream, view: &Arc<Mutex<View>>) -> std::io::Result<()> {
    use std::io::Write;
    // Minimal RFC6455 handshake; the daemon speaks plain text frames.
    let key = "x3JJHMbDL1EzLkh9GBhXDw==";
    write!(
        stream,
        "GET /ws HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )?;
    let mut r = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            break;
        }
    }
    view.lock().unwrap().connected = true;
    loop {
        let Some(payload) = read_frame(&mut r)? else {
            return Ok(());
        };
        if let Ok(v) = serde_json::from_str::<Value>(&payload) {
            apply(&v, view);
        }
    }
}

fn read_frame<R: BufRead + std::io::Read>(r: &mut R) -> std::io::Result<Option<String>> {
    let mut h = [0u8; 2];
    if r.read_exact(&mut h).is_err() {
        return Ok(None);
    }
    let masked = h[1] & 0x80 != 0;
    let mut len = (h[1] & 0x7f) as usize;
    if len == 126 {
        let mut e = [0u8; 2];
        r.read_exact(&mut e)?;
        len = u16::from_be_bytes(e) as usize;
    } else if len == 127 {
        let mut e = [0u8; 8];
        r.read_exact(&mut e)?;
        len = u64::from_be_bytes(e) as usize;
    }
    let mut mask = [0u8; 4];
    if masked {
        r.read_exact(&mut mask)?;
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    if masked {
        for (i, b) in buf.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(Some(String::from_utf8_lossy(&buf).to_string()))
}

/// The ticker keeps its own tiny node table so it can name callers and callees.
static NODES: Mutex<Option<Value>> = Mutex::new(None);

fn apply(v: &Value, view: &Arc<Mutex<View>>) {
    match v.get("type").and_then(Value::as_str) {
        Some("graph") => {
            *NODES.lock().unwrap() = Some(v.clone());
        }
        Some("focus") => {
            let g = NODES.lock().unwrap();
            let Some(g) = g.as_ref() else { return };
            let nodes = g
                .get("nodes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let name_of = |id: u64| -> String {
                nodes
                    .iter()
                    .find(|n| n.get("id").and_then(Value::as_u64) == Some(id))
                    .and_then(|n| n.get("name").and_then(Value::as_str))
                    .unwrap_or("?")
                    .to_string()
            };
            let mut view = view.lock().unwrap();
            view.following = v.get("following").and_then(Value::as_bool).unwrap_or(true);
            view.last_event = v.get("last_event_secs").and_then(Value::as_u64);
            view.agent = v
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or("fs")
                .to_string();
            view.trail = v
                .get("trail")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.get("node").and_then(Value::as_u64))
                        .map(name_of)
                        .collect()
                })
                .unwrap_or_default();
            let active = v.get("active").and_then(Value::as_u64);
            let Some(active) = active else {
                view.active = "—".into();
                return;
            };
            let node = nodes
                .iter()
                .find(|n| n.get("id").and_then(Value::as_u64) == Some(active));
            if let Some(n) = node {
                view.active = n
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string();
                let file = n.get("file").and_then(Value::as_str).unwrap_or("");
                let short: Vec<&str> = file.rsplit('/').take(1).collect();
                view.loc = format!(
                    "{}:{}",
                    short.join(""),
                    n.get("line_start").and_then(Value::as_u64).unwrap_or(0)
                );
            }
            view.crumb = v
                .get("ancestors")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_u64())
                        .map(name_of)
                        .collect::<Vec<_>>()
                        .join(" ▸ ")
                })
                .unwrap_or_default();
            let edges = g
                .get("edges")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut callers = vec![];
            let mut callees = vec![];
            for e in &edges {
                if e.get("kind").and_then(Value::as_str) != Some("calls") {
                    continue;
                }
                let (f, t) = (
                    e.get("from").and_then(Value::as_u64),
                    e.get("to").and_then(Value::as_u64),
                );
                if t == Some(active) {
                    if let Some(f) = f {
                        callers.push(name_of(f))
                    }
                }
                if f == Some(active) {
                    if let Some(t) = t {
                        callees.push(name_of(t))
                    }
                }
            }
            view.resolved = callees.len() as u32;
            view.callers = callers;
            view.callees = callees;
        }
        _ => {}
    }
}
