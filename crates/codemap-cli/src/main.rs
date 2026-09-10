mod explore;
mod hooks;
mod ticker;

use clap::{Parser, Subcommand};
use codemap_graph::NodeKind;
use codemap_index::indexer::index_repo;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "codemap",
    version,
    about = "Live code map for agentic development"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// Answers three things in order: what it is, what you can run, what it will
/// not do behind your back.
fn front_door() -> ExitCode {
    println!("codemap  \u{2014} a live map of your codebase that follows your coding agent");
    println!();
    println!("  start  [path]        index, serve the UI, listen for hooks");
    println!("  explore [path]       full-screen map, vim keys");
    println!("  ticker [path]        10-row pane for tmux beside your agent");
    println!("  status · stop        daemon lifecycle");
    println!();
    println!("  callers · calls · reach · agent   one hop as text; --vimgrep for quickfix");
    println!("  index <path>         summary + call-resolution coverage, no daemon");
    println!("  install-hooks        claude · codex · gemini");
    println!();
    println!("Nothing runs until you start it. No telemetry, loopback only.");
    ExitCode::SUCCESS
}

#[derive(Subcommand)]
enum Cmd {
    /// Index a repository and print a summary
    Index {
        path: PathBuf,
        /// Emit the full graph as JSON
        #[arg(long)]
        json: bool,
        /// Include tests, migrations, examples and generated code
        #[arg(long)]
        all: bool,
    },
    /// List the API routes that can reach a symbol
    Reach {
        path: PathBuf,
        symbol: String,
        /// file:line:col: text, for vim's quickfix list
        #[arg(long)]
        vimgrep: bool,
    },
    /// Start the live daemon for a repo and serve the browser client
    Start {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// 0 picks a free port
        #[arg(long, default_value_t = 7878)]
        port: u16,
    },
    /// Stop the daemon for a repo
    Stop {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Report whether a daemon is running for a repo
    Status {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Internal: reads a hook payload on stdin and forwards it to the daemon
    Hook {
        #[arg(long, default_value = "claude")]
        agent: String,
    },
    /// Write hook config for installed agents
    InstallHooks {
        /// Show what would be written without writing it
        #[arg(long)]
        dry_run: bool,
        /// Remove codemap hooks instead of adding them
        #[arg(long)]
        uninstall: bool,
    },
    /// Live text ticker, for a tmux pane beside your agent
    Ticker {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// What calls a symbol
    Callers {
        path: PathBuf,
        symbol: String,
        /// file:line:col: text, for vim's quickfix list
        #[arg(long)]
        vimgrep: bool,
    },
    /// What a symbol calls
    Calls {
        path: PathBuf,
        symbol: String,
        #[arg(long)]
        vimgrep: bool,
        /// Also list call sites that could not be resolved
        #[arg(long)]
        unresolved: bool,
    },
    /// Full-screen map with vim keys
    Explore {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Open the browser UI, starting the daemon if needed (alias: -ui)
    Ui {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Open the terminal UI, starting the daemon if needed (alias: -term)
    Term {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Where your agent is right now (needs a running daemon)
    Agent {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        vimgrep: bool,
    },
}

fn main() -> ExitCode {
    // `codemap -ui .` and `codemap -term <dir>` are the two commands a developer
    // actually types. Clap has no shape for a single-dash multi-letter flag, so
    // they are rewritten into subcommands before parsing.
    let argv: Vec<String> = std::env::args()
        .map(|a| match a.as_str() {
            "-ui" | "--ui" => "ui".to_string(),
            "-term" | "--term" | "-tui" => "term".to_string(),
            _ => a,
        })
        .collect();
    let Some(cmd) = Cli::parse_from(argv).cmd else {
        return front_door();
    };
    match cmd {
        Cmd::Start { path, port } => cmd_start(path, port),
        Cmd::Stop { path } => cmd_stop(path),
        Cmd::Status { path } => cmd_status(path),
        Cmd::Hook { agent } => cmd_hook(agent),
        Cmd::InstallHooks { dry_run, uninstall } => cmd_install(dry_run, uninstall),
        Cmd::Ticker { path } => cmd_ticker(path),
        Cmd::Callers {
            path,
            symbol,
            vimgrep,
        } => cmd_hops(path, symbol, Dir::In, vimgrep, false),
        Cmd::Calls {
            path,
            symbol,
            vimgrep,
            unresolved,
        } => cmd_hops(path, symbol, Dir::Out, vimgrep, unresolved),
        Cmd::Agent { path, vimgrep } => cmd_agent(path, vimgrep),
        Cmd::Explore { path } => cmd_explore(path, false),
        Cmd::Ui { path } => cmd_ui(path),
        Cmd::Term { path } => cmd_explore(path, true),
        Cmd::Index { path, json, all } => {
            let (g, st) = codemap_index::indexer::index_repo_opts(&path, all);
            if json {
                println!("{}", serde_json::to_string_pretty(&g).unwrap());
            } else {
                let c = |k: NodeKind| g.nodes().iter().filter(|n| n.kind == k).count();
                let internal = st.resolved + st.undeterminable + st.ambiguous;
                let pct = st.internal_resolution_pct();
                println!("{}", path.display());
                println!(
                    "{} modules · {} packages",
                    c(NodeKind::Module),
                    c(NodeKind::Package)
                );
                println!();
                println!(
                    "  classes    {:<8} functions  {}",
                    c(NodeKind::Class),
                    c(NodeKind::Function)
                );
                println!(
                    "  routes     {:<8} edges      {}",
                    c(NodeKind::Route),
                    g.edges().len()
                );
                println!();
                let filled = ((pct / 100.0) * 24.0).round() as usize;
                println!(
                    "  call resolution  {}{}  {:.0}%",
                    "\u{2588}".repeat(filled),
                    "\u{2591}".repeat(24 - filled),
                    pct
                );
                println!("                   of {internal} internal call sites");
                println!();
                println!("  resolved     {:>6}", st.resolved);
                println!(
                    "  ambiguous    {:>6}   >1 candidate, not guessed",
                    st.ambiguous
                );
                println!(
                    "  needs types  {:>6}   calls through variables",
                    st.undeterminable
                );
                println!(
                    "  external     {:>6}   stdlib / dependencies, not counted",
                    st.external
                );
                println!();
                println!("Containment and routes are exact. Only call edges are heuristic.");
                if !all {
                    println!("Tests, generated code and vendored deps skipped. Pass --all to include them.");
                }
            }
            ExitCode::SUCCESS
        }
        Cmd::Reach {
            path,
            symbol,
            vimgrep,
        } => {
            let g = index_repo(&path);
            let n = match pick_symbol(&g, &symbol) {
                Ok(n) => n,
                Err(c) => return c,
            };
            let routes = g.routes_reaching(n.id);
            if routes.is_empty() {
                // Exit 2 so a vim mapping or script can tell "no route found"
                // apart from "symbol not found", and from a real error.
                println!("# no route found — unknown, not unreachable (exit 2)");
                return ExitCode::from(2);
            }
            for r in routes {
                let rn = g.node(r);
                if vimgrep {
                    println!(
                        "{}:{}:1: {} \u{2192} {}",
                        clean(&rn.file),
                        rn.line_start,
                        rn.name,
                        n.name
                    );
                } else {
                    println!(
                        "{:<28} {}",
                        format!("{}:{}", short_path(&rn.file), rn.line_start),
                        rn.name
                    );
                }
            }
            ExitCode::SUCCESS
        }
    }
}

fn abs(p: PathBuf) -> PathBuf {
    p.canonicalize().unwrap_or(p)
}

/// What the UI commands should map: the project, not whatever subdirectory you
/// happened to be standing in. Falls back to the given path outside a repo.
fn project_root(p: PathBuf) -> PathBuf {
    codemap_index::describe::repo_root(&abs(p))
}

fn cmd_start(path: PathBuf, port: u16) -> ExitCode {
    let root = abs(path);
    if codemap_daemon::server::is_running(&root) {
        eprintln!("codemap: already running for {}", root.display());
        return ExitCode::FAILURE;
    }
    let t0 = std::time::Instant::now();
    let (g, st) = codemap_index::indexer::index_repo_with_stats(&root);
    let files = g
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::Module)
        .count();
    println!(
        "\u{2713} indexed   {}   {} files \u{b7} {} nodes \u{b7} {} edges \u{b7} {:.1}s",
        root.display(),
        files,
        g.len(),
        g.edges().len(),
        t0.elapsed().as_secs_f32()
    );
    println!("\u{2713} watching  filesystem \u{2014} works with any agent or by hand");
    let hooks: Vec<String> = hooks::targets()
        .into_iter()
        .map(|t| {
            if t.present {
                format!("{} installed", t.name)
            } else {
                format!("{} not found", t.name)
            }
        })
        .collect();
    println!("\u{2713} hooks     {}", hooks.join("  "));
    println!();
    let internal = st.resolved + st.undeterminable + st.ambiguous;
    println!(
        "Call edges are heuristic \u{2014} {:.0}% of {} internal call sites resolve here.",
        st.internal_resolution_pct(),
        internal
    );
    println!("Tests, generated code and vendored deps skipped. Pass --all to include them.");
    println!();

    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("codemap: {e}");
            return ExitCode::FAILURE;
        }
    };
    let r2 = root.clone();
    let res = rt.block_on(async move { codemap_daemon::server::Daemon::run(r2, port).await });
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("codemap: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_stop(path: PathBuf) -> ExitCode {
    let root = abs(path);
    let sock = codemap_daemon::state::socket_path(&root);
    let _ = std::fs::remove_file(sock.with_extension("port"));
    if std::fs::remove_file(&sock).is_ok() {
        println!("codemap: stopped (socket removed); the process exits on next request");
        ExitCode::SUCCESS
    } else {
        eprintln!("codemap: no daemon for {}", root.display());
        ExitCode::FAILURE
    }
}

fn cmd_status(path: PathBuf) -> ExitCode {
    let root = abs(path);
    if codemap_daemon::server::is_running(&root) {
        let port = codemap_daemon::server::daemon_port(&root);
        println!("\u{25cf} running   {}", root.display());
        println!(
            "  url       http://127.0.0.1:{}",
            port.map(|p| p.to_string()).unwrap_or("?".into())
        );
        if let Some(p) = port {
            if let Some(body) = http_get(p, "/api/focus") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                    let agent = v.get("agent").and_then(|x| x.as_str()).unwrap_or("fs");
                    let name = v
                        .get("qualified_name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    let ago = v.get("ago_secs").and_then(|x| x.as_u64()).unwrap_or(0);
                    if name.is_empty() {
                        println!(
                            "  agent     none yet \u{2014} waiting for a hook or a file change"
                        );
                    } else {
                        println!("  agent     {agent} \u{b7} last hook {ago}s ago");
                        let following =
                            v.get("following").and_then(|x| x.as_bool()).unwrap_or(true);
                        println!(
                            "  focus     {name} \u{b7} {}",
                            if following { "following" } else { "detached" }
                        );
                    }
                }
            }
        }
        println!("  explore   codemap explore {}", root.display());
        println!("  ticker    codemap ticker {}", root.display());
        println!("  stop      codemap stop {}", root.display());
        ExitCode::SUCCESS
    } else {
        println!("\u{25cb} not running   {}", root.display());
        println!();
        println!("  start with  codemap start {}", root.display());
        ExitCode::FAILURE
    }
}

/// Hooks block the agent, so this must be fast and must never fail loudly.
/// With no daemon running it is a single stat() and an immediate exit.
fn cmd_hook(agent: String) -> ExitCode {
    use std::io::Read;
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut root = cwd.as_path();
    // Walk up to the nearest repo that has a daemon.
    let mut found = None;
    loop {
        if codemap_daemon::server::is_running(root) {
            found = Some(root.to_path_buf());
            break;
        }
        match root.parent() {
            Some(p) => root = p,
            None => break,
        }
    }
    let Some(root) = found else {
        // No daemon here: do nothing. This is the common case in every repo
        // where the tool was never started, and must cost almost nothing.
        return ExitCode::SUCCESS;
    };
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        return ExitCode::SUCCESS;
    }
    let payload = match serde_json::from_str::<serde_json::Value>(&buf) {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.insert("codemap_agent".into(), serde_json::Value::String(agent));
            }
            v.to_string()
        }
        Err(_) => return ExitCode::SUCCESS,
    };
    codemap_daemon::server::send_to_daemon(&root, &payload);
    ExitCode::SUCCESS
}

fn cmd_install(dry_run: bool, uninstall: bool) -> ExitCode {
    let mut any = false;
    for t in hooks::targets() {
        if !t.present {
            println!("skip   {:<8} (not installed)", t.name);
            continue;
        }
        any = true;
        if uninstall {
            match hooks::uninstall(t.name) {
                Ok(()) => println!("removed {:<7} {}", t.name, t.config.display()),
                Err(e) => eprintln!("failed  {:<7} {e}", t.name),
            }
            continue;
        }
        if dry_run {
            println!("\n=== {} -> {} ===", t.name, t.config.display());
            println!(
                "{}",
                serde_json::to_string_pretty(&hooks::preview(t.name)).unwrap_or_default()
            );
            continue;
        }
        match hooks::install(t.name) {
            Ok(p) => println!("wrote  {:<8} {}", t.name, p.display()),
            Err(e) => eprintln!("failed {:<8} {e}", t.name),
        }
    }
    if !any {
        println!("no supported agents found (~/.claude, ~/.codex, ~/.gemini)");
    }
    if !dry_run && !uninstall && any {
        println!(
            "\nnote: codex requires you to review and trust the hook definition on first run."
        );
    }
    ExitCode::SUCCESS
}

fn cmd_ticker(path: PathBuf) -> ExitCode {
    let root = abs(path);
    let Some(port) = codemap_daemon::server::daemon_port(&root) else {
        eprintln!(
            "codemap: no daemon for {} — run `codemap start` first",
            root.display()
        );
        return ExitCode::FAILURE;
    };
    match ticker::run(port, root.display().to_string()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("codemap: {e}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    In,
    Out,
}

/// Resolves a symbol name, listing candidates rather than guessing when the
/// bare name is ambiguous.
fn pick_symbol<'a>(
    g: &'a codemap_graph::Graph,
    symbol: &str,
) -> Result<&'a codemap_graph::Node, ExitCode> {
    let matches: Vec<_> = g
        .nodes()
        .iter()
        .filter(|n| n.name == symbol || n.qualified_name == symbol)
        .collect();
    match matches.len() {
        0 => {
            eprintln!("symbol not found: {symbol}");
            Err(ExitCode::FAILURE)
        }
        1 => Ok(matches[0]),
        _ => {
            eprintln!("{} symbols named {symbol} — pick one:", matches.len());
            for m in matches.iter().take(20) {
                eprintln!("  {}  ({}:{})", m.qualified_name, m.file, m.line_start);
            }
            Err(ExitCode::FAILURE)
        }
    }
}

fn cmd_hops(path: PathBuf, symbol: String, dir: Dir, vimgrep: bool, unresolved: bool) -> ExitCode {
    let g = index_repo(&path);
    let n = match pick_symbol(&g, &symbol) {
        Ok(n) => n,
        Err(c) => return c,
    };
    let edges = if dir == Dir::In {
        g.call_sites_into(n.id)
    } else {
        g.call_sites(n.id)
    };
    if edges.is_empty() && !vimgrep {
        println!(
            "no {} indexed for {}",
            if dir == Dir::In { "callers" } else { "calls" },
            n.qualified_name
        );
    }
    for e in &edges {
        // The call site lives in the caller's file either way.
        let site = g.node(e.from);
        let (from, to) = (g.node(e.from), g.node(e.to));
        let (line, col) = if e.line > 0 {
            (e.line, e.col)
        } else {
            (site.line_start, 1)
        };
        if vimgrep {
            println!(
                "{}:{}:{}: {} \u{2192} {}",
                clean(&site.file),
                line,
                col,
                from.name,
                to.name
            );
        } else {
            println!(
                "{:<28} {} \u{2192} {}",
                format!("{}:{}", short_path(clean(&site.file)), line),
                from.name,
                to.name
            );
        }
    }
    if dir == Dir::Out && n.unresolved_calls > 0 {
        if unresolved {
            println!(
                "# {} unresolved call site{} in {} — target unknown, not absent",
                n.unresolved_calls,
                if n.unresolved_calls > 1 { "s" } else { "" },
                n.name
            );
        } else {
            println!(
                "# {} unresolved call site{} omitted — pass --unresolved to list them",
                n.unresolved_calls,
                if n.unresolved_calls > 1 { "s" } else { "" }
            );
        }
    }
    ExitCode::SUCCESS
}

fn clean(p: &str) -> &str {
    p.strip_prefix("./").unwrap_or(p)
}

fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.rsplit('/').take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

/// Asks the running daemon where the agent is. Exits 2 when nothing is known,
/// matching `reach`, so a vim mapping can branch on it.
fn cmd_agent(path: PathBuf, vimgrep: bool) -> ExitCode {
    let root = abs(path);
    let Some(port) = codemap_daemon::server::daemon_port(&root) else {
        eprintln!(
            "no daemon for {} — run `codemap start` first",
            root.display()
        );
        return ExitCode::from(2);
    };
    let body = match http_get(port, "/api/focus") {
        Some(b) => b,
        None => {
            eprintln!("daemon on port {port} did not answer");
            return ExitCode::from(2);
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return ExitCode::from(2),
    };
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
    if name.is_empty() {
        println!("# no agent activity yet");
        return ExitCode::from(2);
    }
    let file = v.get("file").and_then(|x| x.as_str()).unwrap_or("");
    let line = v.get("line").and_then(|x| x.as_u64()).unwrap_or(1);
    let ago = v.get("ago_secs").and_then(|x| x.as_u64()).unwrap_or(0);
    let ago_s = if ago < 60 {
        format!("{ago}s ago")
    } else {
        format!("{}m ago", ago / 60)
    };
    if vimgrep {
        println!("{}:{line}:1: {name} \u{25cf} {ago_s}", clean(file));
    } else {
        println!("{name}  {}:{}  \u{25cf} {ago_s}", short_path(file), line);
    }
    ExitCode::SUCCESS
}

pub(crate) fn http_get(port: u16, path: &str) -> Option<String> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut buf = String::new();
    s.read_to_string(&mut buf).ok()?;
    buf.split_once("\r\n\r\n").map(|(_, b)| b.to_string())
}

/// Starts a daemon for `root` if none is running, and returns its port.
/// Both `-ui` and `-term` are one-command entry points, so neither should make
/// the developer start a daemon by hand first.
fn ensure_daemon(root: &Path) -> Option<u16> {
    if codemap_daemon::server::is_running(root) {
        return codemap_daemon::server::daemon_port(root);
    }
    let exe = std::env::current_exe().ok()?;
    std::process::Command::new(exe)
        .arg("start")
        .arg(root)
        .arg("--port")
        .arg("0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    for _ in 0..80 {
        if codemap_daemon::server::is_running(root) {
            return codemap_daemon::server::daemon_port(root);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    None
}

fn cmd_ui(path: PathBuf) -> ExitCode {
    let root = project_root(path);
    let Some(port) = ensure_daemon(&root) else {
        eprintln!("codemap: could not start a daemon for {}", root.display());
        return ExitCode::FAILURE;
    };
    let url = format!("http://127.0.0.1:{port}");
    println!("codemap  {}", root.display());
    println!("  open      {url}");
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(&url).status();
    ExitCode::SUCCESS
}

fn cmd_explore(path: PathBuf, ensure: bool) -> ExitCode {
    let root = project_root(path);
    let port = if ensure {
        ensure_daemon(&root)
    } else {
        codemap_daemon::server::daemon_port(&root)
    };
    match explore::run(&root, port) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("codemap: {e}");
            ExitCode::FAILURE
        }
    }
}
