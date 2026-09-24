//! Pi extension tool-call events (`tool_call` / `tool_result`).
//!
//! Pi extensions use `@earendil-works/pi-coding-agent` events; see
//! `epr.ts` in nixos-config for the `tool_result` shape this adapter targets.

use serde_json::{Value, json};

use crate::event::{Harness, NormalizedArgs, Phase, ToolEvent, ToolKind};
use crate::policy::{Decision, PolicyOutcome};

pub type PiReply = Value;

pub fn parse(event_name: &str, value: &Value) -> ToolEvent {
    let phase = if event_name.contains("result") || event_name.contains("after") {
        Phase::Post
    } else {
        Phase::Pre
    };
    let tool_name = value.get("toolName").and_then(Value::as_str).unwrap_or("");
    let tool = match tool_name.to_ascii_lowercase().as_str() {
        "read" => ToolKind::Read,
        "write" | "edit" => ToolKind::Edit,
        "bash" | "shell" | "shelltool" => ToolKind::Shell,
        "grep" => ToolKind::Grep,
        "glob" => ToolKind::Glob,
        other => ToolKind::Other(other.to_owned()),
    };
    let mut args = NormalizedArgs::default();
    if let Some(details) = value.get("details") {
        args.raw_input = Some(details.clone());
        if let Some(cmd) = details.get("command").and_then(Value::as_str) {
            args.command = Some(cmd.to_owned());
        }
    }
    let output = value
        .get("content")
        .and_then(|c| c.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        });
    ToolEvent {
        harness: Harness::Pi,
        phase,
        tool,
        args,
        output,
        session_id: value
            .get("sessionId")
            .or_else(|| value.get("toolCallId"))
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
        Decision::Allow => json!({}),
        Decision::Deny { agent_message, .. } => json!({
            "cancel": true,
            "message": agent_message
        }),
        Decision::Rewrite { updated_input, .. } => json!({ "input": updated_input }),
        Decision::ReplaceOutput { text, .. } => json!({
            "content": [{ "type": "text", "text": text }]
        }),
    }
}
