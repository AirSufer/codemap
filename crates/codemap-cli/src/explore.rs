//! `codemap explore` — a full-screen map with vim keys.
//!
//! Three Miller columns (ranger/yazi shape): the parent's children, the
//! current level, and a preview of what is selected. Every entity on screen is
//! one hop away.

use codemap_graph::{Graph, NodeId, NodeKind};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use std::path::Path;
use std::time::{Duration, Instant};

const AGENT: Color = Color::Rgb(242, 180, 90);
const ROUTE: Color = Color::Rgb(111, 211, 160);
const CLASS: Color = Color::Rgb(127, 176, 255);
const MODULE: Color = Color::Rgb(145, 132, 217);
const DEP: Color = Color::Rgb(95, 208, 212);
const WARN: Color = Color::Rgb(238, 138, 99);
const DIM: Color = Color::Rgb(123, 131, 151);
const DIMMER: Color = Color::Rgb(86, 94, 115);
const LINE: Color = Color::Rgb(34, 38, 54);

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Normal,
    Pending,
    Search,
    Command,
    Help,
}

struct Jump {
    via: String,
    node: NodeId,
}

pub struct App {
    g: Graph,
    root: String,
    sel: NodeId,
    mode: Mode,
    count: String,
    input: String,
    hist: Vec<String>,
    jumps: Vec<Jump>,
    jump_at: usize,
    marks: Vec<(char, NodeId)>,
    results: Vec<NodeId>,
    res_i: usize,
    agent: Option<NodeId>,
    agent_ago: u64,
    agent_name: String,
    following: bool,
    port: Option<u16>,
    last_poll: Instant,
    status: String,
    json_on_quit: bool,
}

fn kind_color(k: NodeKind) -> Color {
    match k {
        NodeKind::Route => ROUTE,
        NodeKind::Class => CLASS,
        NodeKind::Module | NodeKind::Package => MODULE,
        NodeKind::Function => DIM,
    }
}
fn glyph(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Route => "\u{25cb}",
        NodeKind::Class => "\u{25c7}",
        NodeKind::Module => "\u{25aa}",
        NodeKind::Package => "\u{25aa}",
        NodeKind::Function => "\u{b7}",
    }
}

impl App {
    fn new(g: Graph, root: String, port: Option<u16>) -> Self {
        let sel = g
            .nodes()
            .iter()
            .find(|n| n.kind == NodeKind::Package || n.kind == NodeKind::Module)
            .map(|n| n.id)
            .unwrap_or(NodeId(0));
        App {
            g,
            root,
            sel,
            mode: Mode::Normal,
            count: String::new(),
            input: String::new(),
            hist: Vec::new(),
            jumps: Vec::new(),
            jump_at: 0,
            marks: Vec::new(),
            results: Vec::new(),
            res_i: 0,
            agent: None,
            agent_ago: 0,
            agent_name: "fs".into(),
            following: true,
            port,
            last_poll: Instant::now() - Duration::from_secs(9),
            status: String::new(),
            json_on_quit: false,
        }
    }

    fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.g.parents_of(id).first().copied()
    }
    fn siblings(&self, id: NodeId) -> Vec<NodeId> {
        match self.parent(id) {
            Some(p) => self.g.children(p),
            None => self
                .g
                .nodes()
                .iter()
                .filter(|n| self.g.parents_of(n.id).is_empty())
                .map(|n| n.id)
                .collect(),
        }
    }
    fn chain(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = vec![id];
        let mut c = id;
        let mut guard = 0;
        while let Some(p) = self.parent(c) {
            out.insert(0, p);
            c = p;
            guard += 1;
            if guard > 40 {
                break;
            }
        }
        out
    }

    fn hop(&mut self, to: NodeId, via: &str) {
        self.jumps.truncate(self.jump_at);
        self.jumps.push(Jump {
            via: via.to_string(),
            node: self.sel,
        });
        self.jump_at = self.jumps.len();
        self.sel = to;
        self.following = false;
    }

    fn poll_agent(&mut self) {
        if self.last_poll.elapsed() < Duration::from_millis(900) {
            return;
        }
        self.last_poll = Instant::now();
        let Some(port) = self.port else { return };
        let Some(body) = crate::http_get(port, "/api/focus") else {
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
            return;
        };
        let qn = v
            .get("qualified_name")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        self.agent_name = v
            .get("agent")
            .and_then(|x| x.as_str())
            .unwrap_or("fs")
            .to_string();
        self.agent_ago = v.get("ago_secs").and_then(|x| x.as_u64()).unwrap_or(0);
        let found = self
            .g
            .nodes()
            .iter()
            .find(|n| n.qualified_name == qn)
            .map(|n| n.id);
        if found != self.agent {
            self.agent = found;
            if self.following {
                if let Some(a) = found {
                    self.sel = a;
                }
            }
        }
    }
}

pub fn run(root: &Path, port: Option<u16>) -> std::io::Result<()> {
    let g = codemap_index::indexer::index_repo(root);
    if g.is_empty() {
        eprintln!("codemap: nothing indexed in {}", root.display());
        return Ok(());
    }
    let mut app = App::new(g, root.display().to_string(), port);

    enable_raw_mode()?;
    std::io::stdout().execute(EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;

    loop {
        app.poll_agent();
        term.draw(|f| render(f, &app))?;
        if event::poll(Duration::from_millis(300))? {
            if let Event::Key(k) = event::read()? {
                if !k.is_press() {
                    continue;
                }
                if handle(&mut app, k) {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    std::io::stdout().execute(LeaveAlternateScreen)?;
    if app.json_on_quit {
        print_json(&app);
    }
    Ok(())
}

fn print_json(app: &App) {
    let n = app.g.node(app.sel);
    let names = |v: Vec<NodeId>| -> Vec<String> {
        v.into_iter()
            .map(|x| app.g.node(x).qualified_name.clone())
            .collect()
    };
    let out = serde_json::json!({
        "node": n.qualified_name,
        "kind": format!("{:?}", n.kind).to_lowercase(),
        "file": n.file, "line": n.line_start,
        "callers": names(app.g.callers(app.sel)),
        "calls": names(app.g.callees(app.sel)),
        "routes": names(app.g.routes_reaching(app.sel)),
        "unresolved_calls": n.unresolved_calls,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}

/* ===================== keys ===================== */

/// Returns true to quit.
fn handle(app: &mut App, k: KeyEvent) -> bool {
    match app.mode {
        Mode::Help => {
            app.mode = Mode::Normal;
            false
        }
        Mode::Pending => {
            app.mode = Mode::Normal;
            pending_key(app, k.code);
            false
        }
        Mode::Search => search_key(app, k),
        Mode::Command => command_key(app, k),
        Mode::Normal => normal_key(app, k),
    }
}

fn take_count(app: &mut App) -> usize {
    let n = app.count.parse::<usize>().unwrap_or(1).max(1);
    app.count.clear();
    n
}

fn normal_key(app: &mut App, k: KeyEvent) -> bool {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Char(c @ '0'..='9') if !(c == '0' && app.count.is_empty()) => {
            app.count.push(c);
        }
        KeyCode::Char('j') | KeyCode::Down => move_sel(app, 1),
        KeyCode::Char('k') | KeyCode::Up => move_sel(app, -1),
        KeyCode::Char('h') | KeyCode::Left => {
            if let Some(p) = app.parent(app.sel) {
                app.sel = p;
                app.following = false;
            }
        }
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
            if let Some(c) = app.g.children(app.sel).first().copied() {
                app.sel = c;
                app.following = false;
            }
        }
        KeyCode::Char('g') => app.mode = Mode::Pending,
        KeyCode::Char('G') => {
            let sibs = app.siblings(app.sel);
            if let Some(&last) = sibs.last() {
                app.sel = last;
            }
        }
        KeyCode::Char('o') if ctrl => jump_walk(app, -1),
        KeyCode::Char('i') if ctrl => jump_walk(app, 1),
        KeyCode::Char(']') => step_caller(app, 1),
        KeyCode::Char('[') => step_caller(app, -1),
        KeyCode::Char('f') => {
            app.following = !app.following;
            if app.following {
                if let Some(a) = app.agent {
                    app.sel = a;
                }
            }
            app.status = format!("follow {}", if app.following { "on" } else { "off" });
        }
        KeyCode::Char('e') => open_editor(app),
        KeyCode::Char('y') => {
            let qn = app.g.node(app.sel).qualified_name.clone();
            codemap_daemon::server::copy_to_clipboard(&qn);
            app.status = format!("yanked {qn}");
        }
        KeyCode::Char('m') => {
            app.mode = Mode::Command;
            app.input = "mark ".into();
        }
        KeyCode::Char('\'') => {
            app.mode = Mode::Command;
            app.input = "'".into();
        }
        KeyCode::Char('/') => {
            app.mode = Mode::Search;
            app.input.clear();
            app.results.clear();
            app.res_i = 0;
        }
        KeyCode::Char(':') => {
            app.mode = Mode::Command;
            app.input.clear();
        }
        KeyCode::Char('?') => app.mode = Mode::Help,
        _ => {}
    }
    false
}

fn move_sel(app: &mut App, dir: i32) {
    let n = take_count(app) as i32;
    let sibs = app.siblings(app.sel);
    if sibs.is_empty() {
        return;
    }
    let cur = sibs.iter().position(|&x| x == app.sel).unwrap_or(0) as i32;
    let next = (cur + dir * n).clamp(0, sibs.len() as i32 - 1);
    app.sel = sibs[next as usize];
    app.following = false;
}

/// `gr` `gc` `gR` `ga` `gf` `gp` `gu` `gg` `gt` `gd` — shown as a which-key
/// popup, so none of it has to be memorised.
fn pending_key(app: &mut App, code: KeyCode) {
    let KeyCode::Char(c) = code else { return };
    match c {
        'r' => hop_first(app, app.g.callers(app.sel), "gr", "no callers indexed"),
        'c' => hop_first(app, app.g.callees(app.sel), "gc", "no calls indexed"),
        'R' => hop_first(
            app,
            app.g.routes_reaching(app.sel),
            "gR",
            "no route found — unknown, not unreachable",
        ),
        'a' => match app.agent {
            Some(a) => {
                app.hop(a, "ga");
                app.following = true;
            }
            None => app.status = "no agent activity yet".into(),
        },
        'f' => {
            let mut c = app.sel;
            while let Some(p) = app.parent(c) {
                if app.g.node(c).kind == NodeKind::Module {
                    break;
                }
                c = p;
            }
            app.hop(c, "gf");
        }
        'p' => {
            let chain = app.chain(app.sel);
            if let Some(&pkg) = chain
                .iter()
                .find(|&&x| app.g.node(x).kind == NodeKind::Package)
            {
                app.hop(pkg, "gp");
            }
        }
        'u' => {
            let n = app.g.node(app.sel);
            app.status = if n.unresolved_calls == 0 {
                "no unresolved call sites here".into()
            } else {
                format!(
                    "{} unresolved call site{} — target unknown, not absent",
                    n.unresolved_calls,
                    if n.unresolved_calls > 1 { "s" } else { "" }
                )
            };
        }
        'g' => {
            let sibs = app.siblings(app.sel);
            if let Some(&first) = sibs.first() {
                app.sel = first;
            }
        }
        'd' => {
            app.status = format!(
                "{}:{}",
                app.g.node(app.sel).file,
                app.g.node(app.sel).line_start
            )
        }
        't' => app.status = "trail lives in the ticker; gt is a placeholder".into(),
        _ => {}
    }
}

fn hop_first(app: &mut App, ids: Vec<NodeId>, via: &str, empty: &str) {
    match ids.first() {
        Some(&id) => app.hop(id, via),
        None => app.status = empty.to_string(),
    }
}

fn step_caller(app: &mut App, dir: i32) {
    let callers = app.g.callers(app.sel);
    if callers.is_empty() {
        app.status = "no callers indexed".into();
        return;
    }
    let i = if dir > 0 { 0 } else { callers.len() - 1 };
    app.hop(callers[i], if dir > 0 { "]" } else { "[" });
}

fn jump_walk(app: &mut App, dir: i32) {
    if dir < 0 && app.jump_at > 0 {
        app.jump_at -= 1;
        app.sel = app.jumps[app.jump_at].node;
    } else if dir > 0 && app.jump_at + 1 < app.jumps.len() {
        app.jump_at += 1;
        app.sel = app.jumps[app.jump_at].node;
    }
}

fn open_editor(app: &mut App) {
    let n = app.g.node(app.sel);
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    app.status = format!("{editor} {}:{}", n.file, n.line_start);
    let _ = std::process::Command::new(&editor)
        .arg(format!("+{}", n.line_start))
        .arg(&n.file)
        .status();
}

fn search_key(app: &mut App, k: KeyEvent) -> bool {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.input.clear();
        }
        KeyCode::Enter => {
            if let Some(&id) = app.results.get(app.res_i) {
                app.hop(id, "/");
            }
            app.mode = Mode::Normal;
            app.input.clear();
        }
        KeyCode::Char('j') if ctrl => {
            app.res_i = (app.res_i + 1).min(app.results.len().saturating_sub(1))
        }
        KeyCode::Char('k') if ctrl => app.res_i = app.res_i.saturating_sub(1),
        KeyCode::Down => app.res_i = (app.res_i + 1).min(app.results.len().saturating_sub(1)),
        KeyCode::Up => app.res_i = app.res_i.saturating_sub(1),
        KeyCode::Backspace => {
            app.input.pop();
            refresh_search(app);
        }
        KeyCode::Char(c) => {
            app.input.push(c);
            refresh_search(app);
        }
        _ => {}
    }
    false
}

fn refresh_search(app: &mut App) {
    let q = app.input.to_lowercase();
    app.res_i = 0;
    if q.is_empty() {
        app.results.clear();
        return;
    }
    let mut hits: Vec<NodeId> = app
        .g
        .nodes()
        .iter()
        .filter(|n| {
            n.name.to_lowercase().contains(&q) || n.qualified_name.to_lowercase().contains(&q)
        })
        .map(|n| n.id)
        .collect();
    hits.sort_by_key(|&i| app.g.node(i).name.len());
    hits.truncate(200);
    app.results = hits;
}

fn command_key(app: &mut App, k: KeyEvent) -> bool {
    match k.code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.input.clear();
        }
        KeyCode::Enter => {
            let line = app.input.clone();
            app.hist.push(line.clone());
            app.input.clear();
            app.mode = Mode::Normal;
            return run_command(app, &line);
        }
        KeyCode::Up => {
            if let Some(h) = app.hist.last() {
                app.input = h.clone();
            }
        }
        KeyCode::Tab => {
            const VERBS: [&str; 8] = [
                "reach", "open", "follow", "detach", "ask", "mark", "json", "trace",
            ];
            if let Some(v) = VERBS.iter().find(|v| v.starts_with(app.input.trim())) {
                app.input = format!("{v} ");
            }
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Char(c) => app.input.push(c),
        _ => {}
    }
    false
}

fn run_command(app: &mut App, line: &str) -> bool {
    let line = line.trim();
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    match verb {
        "q" | "quit" => return true,
        "open" => open_editor(app),
        "follow" => {
            app.following = true;
            if let Some(a) = app.agent {
                app.sel = a;
            }
        }
        "detach" => app.following = false,
        "json" => {
            app.json_on_quit = true;
            app.status = "will dump this node as JSON on quit".into();
        }
        "trace" => {
            let mut out = Vec::new();
            for (i, j) in app.jumps.iter().enumerate() {
                out.push(format!(
                    "{i} {} {}",
                    j.via,
                    app.g.node(j.node).qualified_name
                ));
            }
            app.status = if out.is_empty() {
                "no jumps yet".into()
            } else {
                out.join("  |  ")
            };
        }
        "reach" => {
            let target = rest.trim();
            match app
                .g
                .nodes()
                .iter()
                .find(|n| n.name == target || n.qualified_name == target)
            {
                Some(n) => {
                    let id = n.id;
                    app.hop(id, ":reach");
                }
                None => app.status = format!("symbol not found: {target}"),
            }
        }
        "ask" => {
            let n = app.g.node(app.sel);
            let prompt = format!(
                "{} {} ({}:{})",
                rest.trim(),
                n.qualified_name,
                n.file,
                n.line_start
            );
            codemap_daemon::server::deliver_prompt(&prompt);
            app.status = "typed into the agent pane — not submitted".into();
        }
        "mark" => {
            if let Some(c) = rest.trim().chars().next() {
                app.marks.retain(|(m, _)| *m != c);
                app.marks.push((c, app.sel));
                app.status = format!("mark '{c} set");
            }
        }
        _ if line.starts_with('\'') => {
            if let Some(c) = line.chars().nth(1) {
                if let Some((_, id)) = app.marks.iter().find(|(m, _)| *m == c) {
                    let id = *id;
                    app.hop(id, "'");
                }
            }
        }
        _ => app.status = format!("unknown command: {verb}"),
    }
    false
}

/* ===================== render ===================== */

fn render(f: &mut Frame, app: &App) {
    let rows = Layout::vertical([
        Constraint::Length(1), // breadcrumb
        Constraint::Min(3),    // columns
        Constraint::Length(1), // status
    ])
    .split(f.area());

    breadcrumb(f, rows[0], app);
    columns(f, rows[1], app);
    statusline(f, rows[2], app);

    match app.mode {
        Mode::Pending => which_key(f, rows[1]),
        Mode::Search => search_overlay(f, rows[1], app),
        Mode::Help => help_overlay(f, rows[1]),
        _ => {}
    }
}

fn breadcrumb(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::styled(
        app.root.rsplit('/').next().unwrap_or("repo").to_string(),
        Style::new().fg(DIMMER),
    )];
    for id in app.chain(app.sel) {
        spans.push(Span::styled(" \u{203a} ", Style::new().fg(LINE)));
        let n = app.g.node(id);
        let last = id == app.sel;
        spans.push(Span::styled(
            n.name.clone(),
            if last {
                Style::new().fg(Color::White).bold()
            } else {
                Style::new().fg(DIM)
            },
        ));
    }
    let left = Line::from(spans);
    let right = match app.agent {
        Some(_) => Line::from(vec![
            Span::styled("\u{25cf} ", Style::new().fg(AGENT)),
            Span::styled(app.agent_name.clone(), Style::new().fg(AGENT)),
            Span::styled(
                format!(" is here \u{b7} {}s ", app.agent_ago),
                Style::new().fg(DIMMER),
            ),
        ]),
        None => Line::from(Span::styled(
            "waiting for your agent ",
            Style::new().fg(DIMMER),
        )),
    };
    f.render_widget(Paragraph::new(left), area);
    f.render_widget(Paragraph::new(right).right_aligned(), area);
}

fn col_title(t: String) -> Line<'static> {
    Line::from(Span::styled(
        t,
        Style::new().fg(DIMMER).add_modifier(Modifier::BOLD),
    ))
}

fn node_row(app: &App, id: NodeId, selected: bool, width: usize) -> Line<'static> {
    let n = app.g.node(id);
    let is_agent = app.agent == Some(id);
    let mark = if is_agent { "\u{25cf}" } else { glyph(n.kind) };
    let color = if is_agent { AGENT } else { kind_color(n.kind) };
    let kids = app.g.children(id).len();
    let right = if kids > 0 {
        format!("{kids}")
    } else if n.line_start > 0 && n.kind != NodeKind::Module && n.kind != NodeKind::Package {
        format!(":{}", n.line_start)
    } else {
        String::new()
    };
    let name = n.name.clone();
    let pad = width
        .saturating_sub(name.chars().count() + right.chars().count() + 4)
        .max(1);
    let base = if selected {
        Style::new().fg(Color::White).bg(Color::Rgb(28, 32, 48))
    } else {
        Style::new().fg(if is_agent { AGENT } else { DIM })
    };
    Line::from(vec![
        Span::styled(format!(" {mark} "), Style::new().fg(color)),
        Span::styled(name, base),
        Span::styled(" ".repeat(pad), base),
        Span::styled(right, Style::new().fg(DIMMER)),
    ])
}

fn columns(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::horizontal([
        Constraint::Percentage(22),
        Constraint::Percentage(26),
        Constraint::Percentage(52),
    ])
    .split(area);

    // left: the parent's siblings, i.e. where the current container sits
    let parent = app.parent(app.sel);
    let (left_items, left_sel, left_title) = match parent {
        Some(p) => (
            app.siblings(p),
            Some(p),
            format!("{} \u{b7} {}", app.g.node(p).name, app.g.children(p).len()),
        ),
        None => (Vec::new(), None, String::new()),
    };
    render_list(f, cols[0], left_title, &left_items, left_sel, app, true);

    // middle: the current level
    let sibs = app.siblings(app.sel);
    let mid_title = match parent {
        Some(p) => format!("{} \u{b7} {} symbols", app.g.node(p).name, sibs.len()),
        None => format!(
            "{} \u{b7} {}",
            app.root.rsplit('/').next().unwrap_or(""),
            sibs.len()
        ),
    };
    render_list(f, cols[1], mid_title, &sibs, Some(app.sel), app, false);

    preview(f, cols[2], app);
}

fn render_list(
    f: &mut Frame,
    area: Rect,
    title: String,
    items: &[NodeId],
    sel: Option<NodeId>,
    app: &App,
    dim: bool,
) {
    let inner = Rect {
        x: area.x,
        y: area.y,
        width: area.width.saturating_sub(1),
        height: area.height,
    };
    let w = inner.width as usize;
    let mut lines = vec![col_title(title)];
    let cap = inner.height.saturating_sub(2) as usize;
    let cur = sel
        .and_then(|s| items.iter().position(|&x| x == s))
        .unwrap_or(0);
    let start = cur.saturating_sub(cap.saturating_sub(1));
    for &id in items.iter().skip(start).take(cap) {
        let mut l = node_row(app, id, sel == Some(id), w);
        if dim {
            l = Line::from(
                l.spans
                    .into_iter()
                    .map(|s| {
                        let st = s.style;
                        Span::styled(s.content, st.fg(st.fg.unwrap_or(DIM)))
                    })
                    .collect::<Vec<_>>(),
            );
        }
        lines.push(l);
    }
    // marks live under the middle column, as the design shows
    if !dim && !app.marks.is_empty() && lines.len() + 2 < inner.height as usize {
        lines.push(Line::from(""));
        lines.push(col_title("marks".into()));
        for (c, id) in app.marks.iter().take(4) {
            let n = app.g.node(*id);
            lines.push(Line::from(vec![
                Span::styled(format!(" '{c} "), Style::new().fg(DEP)),
                Span::styled(n.name.clone(), Style::new().fg(DIM)),
                Span::styled(
                    format!("  {}", n.file.rsplit('/').next().unwrap_or("")),
                    Style::new().fg(DIMMER),
                ),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
    // column separator
    let sep = Rect {
        x: area.x + area.width.saturating_sub(1),
        y: area.y,
        width: 1,
        height: area.height,
    };
    f.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::new().fg(LINE)),
        sep,
    );
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for w in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + w.chars().count() + 1 > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(w);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

fn preview(f: &mut Frame, area: Rect, app: &App) {
    use codemap_index::describe;
    let n = app.g.node(app.sel);
    let w = area.width.saturating_sub(3) as usize;
    let is_container = matches!(n.kind, NodeKind::Module | NodeKind::Package);
    let callers = app.g.callers(app.sel);
    let callees = app.g.callees(app.sel);
    let routes = app.g.routes_reaching(app.sel);
    let resolved = callees.len() as u32;
    let total = resolved + n.unresolved_calls;
    let pct = (100 * resolved).checked_div(total).unwrap_or(100);

    let row = |t: &str, spans: Vec<Span<'static>>| {
        let mut v = vec![Span::styled(format!("{t:<10}"), Style::new().fg(DIMMER))];
        v.extend(spans);
        Line::from(v)
    };
    let node_spans = |ids: &[NodeId], take: usize| -> Vec<Span<'static>> {
        if ids.is_empty() {
            return vec![Span::styled("\u{2014}", Style::new().fg(DIMMER))];
        }
        let mut sp = Vec::new();
        for &id in ids.iter().take(take) {
            let m = app.g.node(id);
            sp.push(Span::styled(
                format!("{}  ", m.name),
                Style::new().fg(Color::White),
            ));
        }
        if ids.len() > take {
            sp.push(Span::styled(
                format!("+{} more", ids.len() - take),
                Style::new().fg(DIMMER),
            ));
        }
        sp
    };

    let kindword = match n.kind {
        NodeKind::Function => "fn",
        NodeKind::Class => "type",
        NodeKind::Route => "route",
        NodeKind::Module => "file",
        NodeKind::Package => "crate",
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(n.name.clone(), Style::new().fg(Color::White).bold()),
        Span::styled(
            format!(
                "   {kindword} \u{b7} {}:{}",
                n.file.rsplit('/').next().unwrap_or(""),
                n.line_start
            ),
            Style::new().fg(DIMMER),
        ),
    ])];

    // --- what it does: the doc comment, wrapped, or an honest absence ---
    let source = describe::read_span(&n.file, n.line_start, n.line_end, 400);
    let doc = describe::doc(&n.file, n.line_start, &source);
    lines.push(Line::from(""));
    if doc.is_empty() {
        lines.push(Line::from(Span::styled(
            "no doc comment here",
            Style::new().fg(DIMMER).italic(),
        )));
    } else {
        for l in wrap(&doc, w).into_iter().take(5) {
            lines.push(Line::from(Span::styled(
                l,
                Style::new().fg(Color::Rgb(200, 206, 219)),
            )));
        }
    }

    if is_container {
        // --- a file/crate is described by what it holds and what leans on it ---
        lines.push(Line::from(""));
        lines.push(row(
            "holds",
            vec![Span::styled(
                describe::composition(&app.g, app.sel),
                Style::new().fg(Color::White),
            )],
        ));
        let busy = describe::busiest(&app.g, app.sel, 4);
        if !busy.is_empty() {
            let mut sp = Vec::new();
            for (id, c) in &busy {
                sp.push(Span::styled(
                    app.g.node(*id).name.to_string(),
                    Style::new().fg(Color::White),
                ));
                sp.push(Span::styled(
                    format!(" \u{2190}{c}  "),
                    Style::new().fg(DIMMER),
                ));
            }
            lines.push(row("busiest", sp));
        }
        let rts = describe::routes_within(&app.g, app.sel);
        if !rts.is_empty() {
            lines.push(row("defines", node_spans(&rts, 3)));
        }
        let deps = describe::deps_within(&app.g, app.sel, 6);
        lines.push(row(
            "uses",
            if deps.is_empty() {
                vec![Span::styled(
                    "stdlib and this repo only",
                    Style::new().fg(DIMMER),
                )]
            } else {
                vec![Span::styled(deps.join("  "), Style::new().fg(DEP))]
            },
        ));
    } else {
        // --- a symbol is described by its signature and its neighbours ---
        let sig = describe::signature(&source);
        if !sig.is_empty() {
            lines.push(Line::from(""));
            for l in wrap(&sig, w).into_iter().take(3) {
                lines.push(Line::from(Span::styled(l, Style::new().fg(CLASS))));
            }
        }
        lines.push(Line::from(""));
        lines.push(row("called by", node_spans(&callers, 4)));
        lines.push(row("calls", node_spans(&callees, 4)));
        lines.push(row(
            "routes",
            if routes.is_empty() {
                vec![
                    Span::styled("none found ", Style::new().fg(DIMMER)),
                    Span::styled("\u{2014} unknown, not unreachable", Style::new().fg(DIMMER)),
                ]
            } else {
                node_spans(&routes, 3)
            },
        ));
        if !n.dependencies.is_empty() {
            lines.push(row(
                "uses",
                vec![Span::styled(
                    n.dependencies
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("  "),
                    Style::new().fg(DEP),
                )],
            ));
        }
    }

    // --- how much of this codemap actually knows ---
    lines.push(Line::from(""));
    let bar_w = 18usize;
    let filled = (pct as usize * bar_w) / 100;
    lines.push(Line::from(vec![
        Span::styled(format!("{:<10}", "resolved"), Style::new().fg(DIMMER)),
        Span::styled(
            "\u{2588}".repeat(filled),
            Style::new().fg(if pct >= 50 { ROUTE } else { WARN }),
        ),
        Span::styled("\u{2591}".repeat(bar_w - filled), Style::new().fg(LINE)),
        Span::styled(format!("  {pct}%"), Style::new().fg(DIM)),
        Span::styled(
            if n.unresolved_calls > 0 {
                format!("  {} unresolved", n.unresolved_calls)
            } else {
                String::new()
            },
            Style::new().fg(WARN),
        ),
    ]));

    // --- what you can do from here, so the keys are never a memory test ---
    lines.push(Line::from(""));
    let mut acts: Vec<Span<'static>> = Vec::new();
    let act = |k: &str, t: &str, on: bool| {
        vec![
            Span::styled(
                k.to_string(),
                Style::new().fg(if on { AGENT } else { LINE }),
            ),
            Span::styled(
                format!(" {t}   "),
                Style::new().fg(if on { DIM } else { LINE }),
            ),
        ]
    };
    acts.extend(act("gr", "callers", !callers.is_empty()));
    acts.extend(act("gc", "calls", !callees.is_empty()));
    acts.extend(act("gR", "routes", !routes.is_empty()));
    acts.extend(act("l", "into", !app.g.children(app.sel).is_empty()));
    lines.push(Line::from(acts));
    let mut acts2: Vec<Span<'static>> = Vec::new();
    acts2.extend(act("e", "editor", true));
    acts2.extend(act("y", "yank", true));
    acts2.extend(act(":ask", "prompt agent", true));
    acts2.extend(act("ga", "agent", app.agent.is_some()));
    lines.push(Line::from(acts2));

    // --- source, numbered, only as much as fits ---
    if let Ok(text) = std::fs::read_to_string(&n.file) {
        let budget = area.height.saturating_sub(lines.len() as u16 + 2) as usize;
        if budget > 2 {
            lines.push(Line::from(""));
            let span = if is_container {
                budget.min(20)
            } else {
                budget.min(n.line_end.saturating_sub(n.line_start) as usize + 1)
            };
            for (i, l) in text
                .lines()
                .skip(n.line_start.saturating_sub(1) as usize)
                .take(span)
                .enumerate()
            {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{:>4} ", n.line_start as usize + i),
                        Style::new().fg(LINE),
                    ),
                    Span::styled(
                        l.chars().take(w).collect::<String>(),
                        Style::new().fg(Color::Rgb(174, 182, 198)),
                    ),
                ]));
            }
        }
    }
    let inner = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    };
    f.render_widget(Paragraph::new(lines), inner);
}

fn statusline(f: &mut Frame, area: Rect, app: &App) {
    let mode = match app.mode {
        Mode::Normal | Mode::Pending | Mode::Help => "NORMAL",
        Mode::Search => "SEARCH",
        Mode::Command => "COMMAND",
    };
    let sibs = app.siblings(app.sel);
    let pos = sibs.iter().position(|&x| x == app.sel).unwrap_or(0) + 1;

    let left = if app.mode == Mode::Command {
        Line::from(vec![
            Span::styled(":", Style::new().fg(AGENT)),
            Span::styled(app.input.clone(), Style::new().fg(Color::White)),
            Span::styled("\u{2588}", Style::new().fg(DIMMER)),
            Span::styled(
                "   tab completes \u{b7} \u{2191} history",
                Style::new().fg(DIMMER),
            ),
        ])
    } else if !app.status.is_empty() && app.mode == Mode::Normal {
        Line::from(vec![
            Span::styled(format!(" {mode} "), Style::new().bg(AGENT).fg(Color::Black)),
            Span::styled(format!("  {}", app.status), Style::new().fg(DIM)),
        ])
    } else {
        Line::from(vec![
            Span::styled(format!(" {mode} "), Style::new().bg(AGENT).fg(Color::Black)),
            Span::styled(format!("  {pos}/{}  ", sibs.len()), Style::new().fg(DIM)),
            Span::styled(
                "h parent \u{b7} l into \u{b7} j/k \u{b7} gr callers \u{b7} gc calls \u{b7} gR routes \u{b7} / search \u{b7} : cmd \u{b7} ? help",
                Style::new().fg(DIMMER),
            ),
        ])
    };
    let right = Line::from(vec![
        Span::styled("f follow ", Style::new().fg(DIMMER)),
        Span::styled(
            if app.following { "on" } else { "off" },
            Style::new().fg(if app.following { AGENT } else { DIMMER }),
        ),
        Span::styled(" \u{b7} e $EDITOR \u{b7} q ", Style::new().fg(DIMMER)),
    ]);
    f.render_widget(Paragraph::new(left), area);
    f.render_widget(Paragraph::new(right).right_aligned(), area);
}

fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn framed(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(LINE))
        .title(title)
}

fn which_key(f: &mut Frame, area: Rect) {
    const KEYS: [(&str, &str, &str, &str); 5] = [
        ("d", "definition", "a", "where the agent is"),
        ("r", "callers (references)", "f", "the file"),
        ("c", "calls", "p", "parent crate"),
        ("R", "routes reaching this", "g", "top of list"),
        ("u", "unresolved call sites", "t", "trail (recent touches)"),
    ];
    let mut lines = Vec::new();
    for (k1, d1, k2, d2) in KEYS {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k1}  "), Style::new().fg(AGENT)),
            Span::styled(format!("{d1:<24}"), Style::new().fg(Color::White)),
            Span::styled(format!("{k2}  "), Style::new().fg(AGENT)),
            Span::styled(d2.to_string(), Style::new().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  waiting for second key \u{b7} Esc cancels",
        Style::new().fg(DIMMER),
    )));
    let r = popup(area, 62, lines.len() as u16 + 2);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(framed(" g \u{2026} ")), r);
}

fn search_overlay(f: &mut Frame, area: Rect, app: &App) {
    let r = popup(
        area,
        area.width.saturating_sub(8),
        area.height.saturating_sub(4),
    );
    f.render_widget(Clear, r);
    let inner = Rect {
        x: r.x + 1,
        y: r.y + 1,
        width: r.width - 2,
        height: r.height - 2,
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("/", Style::new().fg(AGENT)),
            Span::styled(app.input.clone(), Style::new().fg(Color::White)),
            Span::styled("\u{2588}", Style::new().fg(DIMMER)),
            Span::styled(
                format!("      {} of {}", app.results.len(), app.g.len()),
                Style::new().fg(DIMMER),
            ),
        ]),
        Line::from(""),
    ];
    for (i, &id) in app
        .results
        .iter()
        .take(inner.height as usize - 4)
        .enumerate()
    {
        let n = app.g.node(id);
        let selected = i == app.res_i;
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", glyph(n.kind)),
                Style::new().fg(kind_color(n.kind)),
            ),
            Span::styled(
                format!("{:<34}", n.name),
                if selected {
                    Style::new().fg(Color::White).bg(Color::Rgb(28, 32, 48))
                } else {
                    Style::new().fg(DIM)
                },
            ),
            Span::styled(
                format!(
                    "{}:{}",
                    n.file.rsplit('/').next().unwrap_or(""),
                    n.line_start
                ),
                Style::new().fg(DIMMER),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " \u{21b5} jump \u{b7} ctrl-j/k move \u{b7} Esc",
        Style::new().fg(DIMMER),
    )));
    f.render_widget(Paragraph::new(lines).block(framed(" SEARCH ")), r);
}

fn help_overlay(f: &mut Frame, area: Rect) {
    let g = |t: &'static str| Line::from(Span::styled(format!("  {t}"), Style::new().fg(DIMMER)));
    let k = |a: &'static str, b: &'static str| {
        Line::from(vec![
            Span::styled(format!("  {a:<14}"), Style::new().fg(AGENT)),
            Span::styled(b.to_string(), Style::new().fg(Color::White)),
        ])
    };
    let lines = vec![
        g("MOVE"),
        k(
            "h j k l",
            "parent \u{b7} down \u{b7} up \u{b7} into (counts work: 5j)",
        ),
        k("gg G", "first \u{b7} last"),
        k("ctrl-o ctrl-i", "back \u{b7} forward through jumps"),
        Line::from(""),
        g("HOP"),
        k("gr gc gR", "callers \u{b7} calls \u{b7} routes"),
        k("ga gf gp", "agent \u{b7} file \u{b7} crate"),
        k("] [", "next \u{b7} prev caller without leaving"),
        Line::from(""),
        g("ACT"),
        k("e", "open in $EDITOR at the line"),
        k("y", "yank qualified name"),
        k("m '", "set mark \u{b7} jump to mark"),
        k("f", "toggle follow"),
        k(": /", "command line \u{b7} fuzzy search"),
    ];
    let r = popup(area, 62, lines.len() as u16 + 2);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(framed(" ? help ")), r);
}
