//! OpenCode plugin hook (`tool.execute.before` / `tool.execute.after`).
//!
//! OpenCode plugins receive `{ tool, input, sessionId, cwd }` on stdin; replies
//! may set `{ allow: false, message }` or `{ output: string }` on after hooks.

use serde_json::{Value, json};

use crate::event::{Harness, NormalizedArgs, Phase, ToolEvent, ToolKind};
use crate::policy::{Decision, PolicyOutcome};

pub type OpencodeReply = Value;

pub fn parse(event_name: &str, value: &Value) -> ToolEvent {
    let phase = if event_name.contains("after") {
        Phase::Post
    } else {
        Phase::Pre
    };
    let tool_name = value
        .get("tool")
        .or_else(|| value.get("toolName"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let tool = match tool_name.to_ascii_lowercase().as_str() {
        "read" => ToolKind::Read,
        "write" | "edit" => ToolKind::Write,
        "bash" | "shell" => ToolKind::Shell,
        "grep" => ToolKind::Grep,
        "glob" => ToolKind::Glob,
        other => ToolKind::Other(other.to_owned()),
    };
    let input = value
        .get("input")
        .cloned()
        .or_else(|| value.get("args").cloned());
    let mut args = NormalizedArgs::default();
    args.raw_input = input.clone();
    if let Some(obj) = input.and_then(|v| v.as_object().cloned()) {
        args.path = obj.get("path").and_then(|v| v.as_str()).map(str::to_owned);
        args.command = obj
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        args.pattern = obj
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
    }
    let output = value
        .get("output")
        .and_then(Value::as_str)
        .map(str::to_owned);
    ToolEvent {
        harness: Harness::Opencode,
        phase,
        tool,
        args,
        output,
        session_id: value
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        cwd: value
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or(".")
            .to_owned(),
    }
}

pub fn render(_event_name: &str, outcome: &PolicyOutcome) -> Value {
    match &outcome.applied {
        Decision::Allow => json!({ "allow": true }),
        Decision::Deny { agent_message, .. } => json!({
            "allow": false,
            "message": agent_message
        }),
        Decision::Rewrite { updated_input, .. } => json!({
            "allow": true,
            "input": updated_input
        }),
        Decision::ReplaceOutput { text, .. } => json!({ "output": text }),
    }
}
