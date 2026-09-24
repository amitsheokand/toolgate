//! MCP server: the `read` gate over stdio (HTTP later if needed).

use std::path::PathBuf;

use rmcp::{
    ErrorData as McpError, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};

use crate::read;

fn check_root(root: &str) -> Result<PathBuf, McpError> {
    let path = PathBuf::from(root);
    if !path.is_absolute() {
        return Err(McpError::invalid_params(
            format!("root must be absolute: {root}"),
            None,
        ));
    }
    if !path.is_dir() {
        return Err(McpError::invalid_params(
            format!("root is not a directory: {root}"),
            None,
        ));
    }
    Ok(path)
}

fn invalid(e: impl std::fmt::Display) -> McpError {
    McpError::invalid_params(e.to_string(), None)
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RunParams {
    /// Absolute workspace root (working directory).
    root: String,
    /// Program to execute (argv-direct, no shell).
    program: String,
    /// Arguments.
    args: Option<Vec<String>>,
    /// Wall-clock budget in seconds (default 120).
    timeout: Option<u64>,
    /// Ask the Jev safety gate first (needs `TYPESAFE_API_KEY`).
    /// Refusals fail closed: no key, no run.
    gate: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EditParams {
    /// Absolute workspace root.
    root: String,
    /// File to edit, relative to root or absolute.
    path: String,
    /// Exact text to replace (must match once unless `all`).
    old: String,
    /// Replacement text.
    new: String,
    /// Replace every occurrence instead of requiring exactly one.
    all: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ReadParams {
    /// Absolute workspace root.
    root: String,
    /// File to read, relative to root or absolute.
    path: String,
    /// 1-based anchor line: returns a window around it (default radius 100).
    line: Option<u64>,
    /// Window radius around `line` (default 100).
    radius: Option<u64>,
    /// Explicit 1-based inclusive range start (needs `end`).
    start: Option<u64>,
    /// Explicit 1-based inclusive range end (needs `start`).
    end: Option<u64>,
}

#[derive(Clone)]
pub struct ToolGate {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl ToolGate {
    /// Create the server.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Bounded file read. `line` returns a window around it (~200 lines, never the whole file); without `line`, files over 400 lines require explicit `start`/`end`. Escapes, binaries, and generated files are refused. Cite what you read."
    )]
    async fn read(
        &self,
        Parameters(p): Parameters<ReadParams>,
    ) -> Result<CallToolResult, McpError> {
        let root = check_root(&p.root)?;
        let range = match (p.start, p.end) {
            (None, None) => None,
            (Some(start), Some(end)) => Some((start, end)),
            _ => {
                return Err(McpError::invalid_params(
                    "`start` and `end` must be set together",
                    None,
                ));
            }
        };
        let hit = read::read(
            &root,
            &p.path,
            p.line,
            p.radius.unwrap_or(read::DEFAULT_RADIUS),
            range,
            read::WHOLE_FILE_LIMIT_LINES,
        )
        .map_err(invalid)?;
        let mut out = format!(
            "{}:{}-{} of {} lines\n",
            hit.path.display(),
            hit.start,
            hit.end,
            hit.total
        );
        out.push_str(&hit.text.join("\n"));
        Ok(CallToolResult::success(vec![ContentBlock::text(out)]))
    }

    #[tool(
        description = "Exact-string edit that returns its diff. `old` must match once (or pass `all`); the response is the bounded diff, so no re-read is needed to check the write."
    )]
    async fn edit(
        &self,
        Parameters(p): Parameters<EditParams>,
    ) -> Result<CallToolResult, McpError> {
        let root = check_root(&p.root)?;
        let hit = crate::edit::edit(&root, &p.path, &p.old, &p.new, p.all.unwrap_or(false))
            .map_err(invalid)?;
        let mut out = format!(
            "{}:{}-{} ({} applied)\n",
            hit.path.display(),
            hit.start,
            hit.end,
            hit.applied
        );
        out.push_str(&hit.diff.join("\n"));
        Ok(CallToolResult::success(vec![ContentBlock::text(out)]))
    }

    #[tool(
        description = "Bounded command execution: kills on timeout, head-truncates output. Returns exit code, verdict, and capped stdout/stderr. argv-direct, no shell. `gate` asks the Jev safety Noul first (needs TYPESAFE_API_KEY); refusals fail closed."
    )]
    async fn run(&self, Parameters(p): Parameters<RunParams>) -> Result<CallToolResult, McpError> {
        let root = check_root(&p.root)?;
        if p.gate.unwrap_or(false) {
            let gate = crate::gate::Gate::from_env().map_err(|e| {
                McpError::internal_error(format!("safety gate unavailable: {e}"), None)
            })?;
            let root_str = root.to_string_lossy().into_owned();
            let score = gate
                .judge(&p.program, &p.args.clone().unwrap_or_default(), &root_str)
                .await
                .map_err(|e| McpError::internal_error(format!("safety gate failed: {e}"), None))?;
            match crate::gate::decide(score, &crate::gate::Policy::default()) {
                crate::gate::Verdict::Allow => {}
                crate::gate::Verdict::Ask(reason) => {
                    return Err(McpError::internal_error(
                        format!("safety gate withholds run ({reason})"),
                        None,
                    ));
                }
                crate::gate::Verdict::Block(reason) => {
                    return Err(McpError::internal_error(
                        format!("safety gate refused run ({reason})"),
                        None,
                    ));
                }
            }
        }
        let hit = crate::run::run(
            &root,
            &p.program,
            &p.args.unwrap_or_default(),
            p.timeout.unwrap_or(crate::run::DEFAULT_TIMEOUT_SECS),
            crate::run::OUTPUT_CAP_BYTES,
        )
        .map_err(|e| match e {
            crate::run::Error::Timeout(_) => McpError::internal_error(e.to_string(), None),
            other => invalid(other),
        })?;
        let mut out = format!(
            "exit={} timed_out={} elapsed_ms={}\n",
            hit.code, hit.timed_out, hit.elapsed_ms
        );
        if !hit.stdout.is_empty() {
            out.push_str("--- stdout ---\n");
            out.push_str(&hit.stdout);
        }
        if !hit.stderr.is_empty() {
            out.push_str("\n--- stderr ---\n");
            out.push_str(&hit.stderr);
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(out)]))
    }
}

impl Default for ToolGate {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_handler]
impl rmcp::ServerHandler for ToolGate {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info.version = env!("CARGO_PKG_VERSION").to_owned();
        info.instructions = Some(
            "Harness-side guardrails. `read` returns bounded windows, never \
             unbounded whole-file dumps: pass `line` for a window around it, \
             or `start`+`end` for an explicit range. Files over 400 lines \
             require one of the two."
                .into(),
        );
        info
    }
}

/// Serve over stdio (local MCP clients).
///
/// Transport failures surface as I/O errors; the read logic itself is
/// infallible past this point.
///
/// # Errors
///
/// Returns [`std::io::Error`] when transport setup fails.
pub async fn serve_stdio() -> std::io::Result<()> {
    fn to_io(e: impl std::fmt::Display) -> std::io::Error {
        std::io::Error::other(e.to_string())
    }
    ToolGate::new()
        .serve(stdio())
        .await
        .map_err(to_io)?
        .waiting()
        .await
        .map_err(to_io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_router_lists_all_gates() {
        let map = &ToolGate::new().tool_router.map;
        for tool in ["read", "edit", "run"] {
            assert!(map.contains_key(tool), "{tool}");
        }
    }

    #[tokio::test]
    async fn edit_tool_applies_and_returns_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "alpha\nbeta\n").expect("write");
        let result = ToolGate::new()
            .edit(Parameters(EditParams {
                root: dir.path().to_string_lossy().into_owned(),
                path: "a.txt".into(),
                old: "beta".into(),
                new: "BETA".into(),
                all: None,
            }))
            .await
            .expect("edit");
        let text = serde_json::to_value(result).expect("response");
        let body = text["content"][0]["text"].as_str().unwrap();
        assert!(body.contains("(1 applied)"), "{body}");
        assert!(body.contains("+BETA"), "{body}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).expect("read"),
            "alpha\nBETA\n"
        );
    }

    #[tokio::test]
    async fn run_tool_reports_exit_and_truncates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = ToolGate::new()
            .run(Parameters(RunParams {
                root: dir.path().to_string_lossy().into_owned(),
                program: "echo".into(),
                args: Some(vec!["hi".into()]),
                timeout: Some(10),
                gate: None,
            }))
            .await
            .expect("run");
        let text = serde_json::to_value(result).expect("response");
        let body = text["content"][0]["text"].as_str().unwrap();
        assert!(body.contains("exit=0"), "{body}");
        assert!(body.contains("hi"), "{body}");
    }

    #[tokio::test]
    async fn read_tool_windows_and_refuses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body: String = (1..=500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("a.txt"), body).expect("write");
        let root = dir.path().to_string_lossy().into_owned();
        let body_of = |result: CallToolResult| {
            serde_json::to_value(result).expect("response")["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        // Window around a line.
        let out = ToolGate::new()
            .read(Parameters(ReadParams {
                root: root.clone(),
                path: "a.txt".into(),
                line: Some(250),
                radius: None,
                start: None,
                end: None,
            }))
            .await
            .expect("read");
        let body = body_of(out);
        assert!(
            body.starts_with("a.txt:150-350 of 500 lines")
                || body.contains(":150-350 of 500 lines"),
            "{body}"
        );
        // Whole-file without range: refused over the limit.
        let err = ToolGate::new()
            .read(Parameters(ReadParams {
                root,
                path: "a.txt".into(),
                line: None,
                radius: None,
                start: None,
                end: None,
            }))
            .await
            .expect_err("must refuse");
        assert!(err.message.contains("start"), "{err:?}");
    }
}
