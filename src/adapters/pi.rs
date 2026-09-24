//! Pi extension tool-call events (`tool_call` / `tool_result`).
//!
//! Pi extensions use `@earendil-works/pi-coding-agent` events; see
//! `epr.ts` in nixos-config for `commandOf` (`event.input`) and `tool_result`
//! return shape `{ content, details }`.

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
    let input = value.get("input").cloned();
    args.raw_input = input.clone();
    if let Some(obj) = input.and_then(|v| v.as_object().cloned()) {
        args.path = obj.get("path").and_then(|v| v.as_str()).map(str::to_owned);
        args.offset = obj.get("offset").and_then(|v| v.as_u64());
        args.limit = obj.get("limit").and_then(|v| v.as_u64());
        args.command = obj
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        if let Some(tr) = obj.get("then_run").and_then(|v| v.as_str()) {
            if args.command.is_none() {
                args.command = Some(tr.to_owned());
            }
        }
    }
    if let Some(details) = value.get("details") {
        if args.raw_input.is_none() {
            args.raw_input = Some(details.clone());
        }
        if args.command.is_none() {
            if let Some(cmd) = details.get("command").and_then(Value::as_str) {
                args.command = Some(cmd.to_owned());
            }
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

pub fn render(event_name: &str, outcome: &PolicyOutcome) -> Value {
    if event_name == "tool_call" {
        return render_tool_call(outcome);
    }
    if event_name == "tool_result" {
        return render_tool_result(outcome);
    }
    render_tool_call(outcome)
}

fn render_tool_call(outcome: &PolicyOutcome) -> Value {
    match &outcome.applied {
        Decision::Allow => json!({}),
        Decision::Deny { agent_message, .. } => json!({
            "block": true,
            "reason": agent_message
        }),
        Decision::Rewrite { updated_input, .. } => json!({ "input": updated_input }),
        Decision::ReplaceOutput { .. } => json!({}),
    }
}

fn render_tool_result(outcome: &PolicyOutcome) -> Value {
    match &outcome.applied {
        Decision::Allow => json!({}),
        Decision::Deny { .. } => json!({}),
        Decision::Rewrite { .. } => json!({}),
        Decision::ReplaceOutput { text, .. } => json!({
            "content": [{ "type": "text", "text": text }]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_from_input() {
        let payload = json!({
            "toolName": "bash",
            "input": { "command": "rm -rf /" }
        });
        let ev = parse("tool_call", &payload);
        assert_eq!(ev.args.command.as_deref(), Some("rm -rf /"));
    }

    #[test]
    fn deny_blocks_on_tool_call() {
        let outcome = PolicyOutcome {
            raw: Decision::Deny {
                agent_message: "blocked".into(),
                rule_id: "shell.rules_deny".into(),
            },
            applied: Decision::Deny {
                agent_message: "blocked".into(),
                rule_id: "shell.rules_deny".into(),
            },
            rule_id: Some("shell.rules_deny".into()),
        };
        let reply = render("tool_call", &outcome);
        assert_eq!(reply["block"], true);
        assert!(reply.get("cancel").is_none());
    }
}
