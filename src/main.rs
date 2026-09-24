use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum GateCli {
    Off,
    Jev,
}

impl From<GateCli> for toolgate::gate::GateMode {
    fn from(value: GateCli) -> Self {
        match value {
            GateCli::Off => Self::Off,
            GateCli::Jev => Self::Jev,
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "toolgate", version, about = "Harness-side guardrails")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum HarnessCli {
    Cursor,
    Muse,
    Opencode,
    Pi,
}

impl From<HarnessCli> for toolgate::event::Harness {
    fn from(value: HarnessCli) -> Self {
        match value {
            HarnessCli::Cursor => Self::Cursor,
            HarnessCli::Muse => Self::Muse,
            HarnessCli::Opencode => Self::Opencode,
            HarnessCli::Pi => Self::Pi,
        }
    }
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
    Serve {
        /// Use stdio transport (the only transport; kept for uniformity).
        #[arg(long)]
        stdio: bool,
        /// Safety gate for `run`: `off` (default) or `jev` (also `TOOLGATE_GATE`).
        #[arg(long, value_enum, default_value = "off")]
        gate: GateCli,
    },
    /// Bounded command execution (timeout + output budget).
    Run {
        /// Program to execute (argv-direct, no shell).
        program: String,
        /// Arguments.
        #[arg(last = true)]
        args: Vec<String>,
        /// Workspace root (working directory).
        #[arg(long, default_value = ".")]
        root: std::path::PathBuf,
        /// Wall-clock budget in seconds (default 120).
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Harness hooks (stdio JSON in/out).
    Hook {
        /// Harness adapter (`cursor`, `muse`, `opencode`, `pi`).
        #[arg(long, value_enum)]
        harness: Option<HarnessCli>,
        /// Hook event name (`preToolUse`, `postToolUse`, `PreToolUse`, …).
        #[arg(long, default_value = "preToolUse")]
        event: String,
        /// Legacy alias for Cursor read cap (`preToolUse`).
        #[arg(long, hide = true)]
        cursor_read: bool,
    },
    /// Exact-string edit that returns its diff (no re-read needed).
    Edit {
        /// File to edit.
        path: std::path::PathBuf,
        /// Workspace root escapes are checked against (default: cwd).
        #[arg(long, default_value = ".")]
        root: std::path::PathBuf,
        /// Exact text to replace (must match once, or use `--all`).
        #[arg(long)]
        old: String,
        /// Replacement text.
        #[arg(long)]
        new: String,
        /// Replace every occurrence instead of requiring exactly one.
        #[arg(long)]
        all: bool,
    },
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
            let numbered = toolgate::read::format_numbered_lines(hit.start, &hit.text);
            println!("{}", numbered.join("\n"));
        }
        Command::Hook {
            harness,
            event,
            cursor_read,
        } => {
            let harness = if cursor_read {
                toolgate::event::Harness::Cursor
            } else {
                harness
                    .ok_or_else(|| anyhow::anyhow!("--harness is required (or use --cursor-read)"))?
                    .into()
            };
            let event = if cursor_read {
                "preToolUse".to_owned()
            } else {
                event
            };
            if toolgate::hook::hook_stdio(harness, &event).is_err() {
                let allow = toolgate::adapters::allow_reply(harness, &event);
                println!(
                    "{}",
                    serde_json::to_string(&allow)
                        .unwrap_or_else(|_| r#"{"permission":"allow"}"#.into())
                );
            }
            return Ok(());
        }
        Command::Serve { gate, .. } => {
            let mode = toolgate::gate::GateMode::from_env_or(gate.into());
            toolgate::mcp::serve_stdio(mode).await?;
        }
        Command::Run {
            program,
            args,
            root,
            timeout,
        } => {
            let hit = toolgate::run::run(
                &root,
                &program,
                &args,
                timeout.unwrap_or(toolgate::run::DEFAULT_TIMEOUT_SECS),
                toolgate::run::OUTPUT_CAP_BYTES,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "exit={} timed_out={} elapsed_ms={}",
                hit.code, hit.timed_out, hit.elapsed_ms
            );
            if !hit.stdout.is_empty() {
                println!("--- stdout ---\n{}", hit.stdout);
            }
            if !hit.stderr.is_empty() {
                println!("--- stderr ---\n{}", hit.stderr);
            }
        }
        Command::Edit {
            path,
            root,
            old,
            new,
            all,
        } => {
            let raw = path.to_string_lossy().into_owned();
            let hit = toolgate::edit::edit(&root, &raw, &old, &new, all)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "{}:{}-{} ({} applied)",
                hit.path.display(),
                hit.start,
                hit.end,
                hit.applied
            );
            println!("{}", hit.diff.join("\n"));
        }
    }
    Ok(())
}
