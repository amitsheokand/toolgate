//! Normalized harness tool events (adapter input to the policy engine).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which harness produced the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Cursor,
    Muse,
    Opencode,
    Pi,
}

/// Hook phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Pre,
    Post,
}

/// Normalized tool kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Read,
    Edit,
    Write,
    Shell,
    Grep,
    Glob,
    Mcp { server: String, tool: String },
    Other(String),
}

/// Normalized arguments extracted from harness payloads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedArgs {
    pub path: Option<String>,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub pattern: Option<String>,
    /// Raw tool input when useful for rewrites.
    pub raw_input: Option<Value>,
}

/// One policy decision input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolEvent {
    pub harness: Harness,
    pub phase: Phase,
    pub tool: ToolKind,
    pub args: NormalizedArgs,
    pub output: Option<String>,
    pub session_id: String,
    pub cwd: String,
}
