use clap::{Parser, Subcommand};
use codemap_graph::NodeKind;
use codemap_index::indexer::{index_repo, index_repo_with_stats};
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
    },
    /// List the API routes that can reach a symbol
    Reach { path: PathBuf, symbol: String },
}

fn main() -> ExitCode {
    match Cli::parse().cmd {
        Cmd::Index { path, json } => {
            let (g, st) = index_repo_with_stats(&path);
            if json {
                println!("{}", serde_json::to_string_pretty(&g).unwrap());
            } else {
                let c = |k: NodeKind| g.nodes().iter().filter(|n| n.kind == k).count();
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
