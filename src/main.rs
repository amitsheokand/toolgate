use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "toolgate", version, about = "Harness-side guardrails")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Bounded file read (windows by default, never unbounded dumps).
    Read {
        /// File to read.
        path: std::path::PathBuf,
        /// Workspace root escapes are checked against (default: cwd).
        #[arg(long, default_value = ".")]
        root: std::path::PathBuf,
        /// 1-based anchor line: window around it.
        #[arg(long)]
        line: Option<u64>,
        /// Window radius around `--line` (default 100).
        #[arg(long)]
        radius: Option<u64>,
        /// Explicit range start (1-based, needs `--end`).
        #[arg(long)]
        start: Option<u64>,
        /// Explicit range end (inclusive, needs `--start`).
        #[arg(long)]
        end: Option<u64>,
    },
    /// Serve the MCP server over stdio.
    Serve {},
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Read {
            path,
            root,
            line,
            radius,
            start,
            end,
        } => {
            let range = match (start, end) {
                (None, None) => None,
                (Some(start), Some(end)) => Some((start, end)),
                _ => anyhow::bail!("--start and --end must be set together"),
            };
            let raw = path.to_string_lossy().into_owned();
            let hit = toolgate::read::read(
                &root,
                &raw,
                line,
                radius.unwrap_or(toolgate::read::DEFAULT_RADIUS),
                range,
                toolgate::read::WHOLE_FILE_LIMIT_LINES,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "{}:{}-{} of {} lines",
                hit.path.display(),
                hit.start,
                hit.end,
                hit.total
            );
            println!("{}", hit.text.join("\n"));
        }
        Command::Serve {} => {
            toolgate::mcp::serve_stdio().await?;
        }
    }
    Ok(())
}
