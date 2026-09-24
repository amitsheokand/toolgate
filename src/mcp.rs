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
    fn tool_router_lists_read() {
        assert!(ToolGate::new().tool_router.map.contains_key("read"));
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
