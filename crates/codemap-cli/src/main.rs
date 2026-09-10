mod hooks;
mod ticker;

use clap::{Parser, Subcommand};
use codemap_graph::NodeKind;
use codemap_index::indexer::index_repo;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "codemap",
    version,
    about = "Live code map for agentic development"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
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
    Reach { path: PathBuf, symbol: String },
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
}

fn main() -> ExitCode {
    match Cli::parse().cmd {
        Cmd::Start { path, port } => cmd_start(path, port),
        Cmd::Stop { path } => cmd_stop(path),
        Cmd::Status { path } => cmd_status(path),
        Cmd::Hook { agent } => cmd_hook(agent),
        Cmd::InstallHooks { dry_run, uninstall } => cmd_install(dry_run, uninstall),
        Cmd::Ticker { path } => cmd_ticker(path),
        Cmd::Index { path, json, all } => {
            let (g, st) = codemap_index::indexer::index_repo_opts(&path, all);
            if json {
                println!("{}", serde_json::to_string_pretty(&g).unwrap());
            } else {
                let c = |k: NodeKind| g.nodes().iter().filter(|n| n.kind == k).count();
                println!("packages  {}", c(NodeKind::Package));
                println!("modules   {}", c(NodeKind::Module));
                println!("classes   {}", c(NodeKind::Class));
                println!("functions {}", c(NodeKind::Function));
                println!("routes    {}", c(NodeKind::Route));
                println!("edges     {}", g.edges().len());
                println!();
                println!("call sites          {}", st.calls_total);
                println!("  resolved          {}", st.resolved);
                println!(
                    "  external/stdlib   {}  (callee not in this repo)",
                    st.external
                );
                println!(
                    "  ambiguous         {}  (>1 candidate, not guessed)",
                    st.ambiguous
                );
                println!(
                    "  undeterminable    {}  (needs type inference)",
                    st.undeterminable
                );
                println!(
                    "internal resolution {:.1}%  (excludes external/stdlib)",
                    st.internal_resolution_pct()
                );
            }
            ExitCode::SUCCESS
        }
        Cmd::Reach { path, symbol } => {
            let g = index_repo(&path);
            let matches: Vec<_> = g
                .nodes()
                .iter()
                .filter(|n| n.name == symbol || n.qualified_name == symbol)
                .collect();
            let n = match matches.len() {
                0 => {
                    eprintln!("symbol not found: {symbol}");
                    return ExitCode::FAILURE;
                }
                1 => matches[0],
                _ => {
                    // Never silently pick one. Show the candidates so the caller
                    // can re-run with a qualified name.
                    eprintln!("{} symbols named {symbol}; qualify one of:", matches.len());
                    for m in matches.iter().take(20) {
                        eprintln!("  {}  ({}:{})", m.qualified_name, m.file, m.line_start);
                    }
                    if matches.len() > 20 {
                        eprintln!("  ... and {} more", matches.len() - 20);
                    }
                    return ExitCode::FAILURE;
                }
            };
            let routes = g.routes_reaching(n.id);
            if routes.is_empty() {
                println!(
                    "{} is not reachable from any detected route",
                    n.qualified_name
                );
            } else {
                println!("{} is reachable from:", n.qualified_name);
                for r in routes {
                    println!("  {}", g.node(r).name);
                }
            }
            ExitCode::SUCCESS
        }
    }
}

fn abs(p: PathBuf) -> PathBuf {
    p.canonicalize().unwrap_or(p)
}

fn cmd_start(path: PathBuf, port: u16) -> ExitCode {
    let root = abs(path);
    if codemap_daemon::server::is_running(&root) {
        eprintln!("codemap: already running for {}", root.display());
        return ExitCode::FAILURE;
    }
    println!("codemap: indexing {}", root.display());
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("codemap: {e}");
            return ExitCode::FAILURE;
        }
    };
    let r2 = root.clone();
    let res = rt.block_on(async move {
        println!("codemap: ticker -> codemap ticker {}", r2.display());
        codemap_daemon::server::Daemon::run(r2, port).await
    });
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
        println!(
            "running   {}\nurl       http://127.0.0.1:{}",
            root.display(),
            port.map(|p| p.to_string()).unwrap_or("?".into())
        );
        ExitCode::SUCCESS
    } else {
        println!("not running for {}", root.display());
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
    match ticker::run(port) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("codemap: {e}");
            ExitCode::FAILURE
        }
    }
}
