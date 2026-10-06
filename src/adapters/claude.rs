//! Claude Code command hooks (`PreToolUse` / `PostToolUse` in `~/.claude/settings.json`).
//!
//! Same reply shape as Muse (Claude Code–compatible hook JSON). Differences on
//! the way in: the session id is `session_id` (not per-call `tool_use_id`),
//! MCP tools are named `mcp__<server>__<tool>`, and post output is
//! `tool_response`, a string or an object.

use serde_json::{Value, json};

use crate::adapters::{cursor, muse};
use crate::event::ToolKind;
use crate::policy::PolicyOutcome;

pub fn parse(event_name: &str, value: &Value) -> crate::event::ToolEvent {
    let mut ev = muse::parse(event_name, value);
    ev.harness = crate::event::Harness::Claude;
    if let Some(id) = value.get("session_id").and_then(Value::as_str) {
        id.clone_into(&mut ev.session_id);
    }
    let tool_name = value.get("tool_name").and_then(Value::as_str).unwrap_or("");
    if let Some((server, tool)) = tool_name
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
    {
        ev.tool = ToolKind::Mcp {
            server: server.to_owned(),
            tool: tool.to_owned(),
        };
    }
    if event_name == "PostToolUse" {
        ev.output = value.get("tool_response").map(response_text);
    }
    ev
}

/// MCP results arrive as `[{type: "text", text}]`; Bash as `{stdout, stderr}`.
fn response_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(o) if o.contains_key("stdout") => {
            let field = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("");
            format!("{}{}", field("stdout"), field("stderr"))
        }
        other => other.to_string(),
    }
}

pub fn render(event_name: &str, outcome: &PolicyOutcome) -> Value {
    match event_name {
        "PreToolUse" | "PostToolUse" => muse::render(event_name, outcome),
        other => cursor::render(other, outcome),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_name_splits_server_and_tool() {
        let ev = parse(
            "PreToolUse",
            &json!({"tool_name": "mcp__one-grep__rg", "tool_input": {}, "session_id": "s1"}),
        );
        assert_eq!(
            ev.tool,
            ToolKind::Mcp {
                server: "one-grep".into(),
                tool: "rg".into()
            }
        );
        assert_eq!(ev.session_id, "s1");
    }

    #[test]
    fn session_id_beats_tool_use_id() {
        let ev = parse(
            "PreToolUse",
            &json!({"tool_name": "Read", "tool_input": {"file_path": "a"}, "session_id": "s1", "tool_use_id": "t9"}),
        );
        assert_eq!(ev.session_id, "s1");
        assert_eq!(ev.tool, ToolKind::Read);
    }

    #[test]
    fn post_output_from_tool_response_shapes() {
        let mcp = parse(
            "PostToolUse",
            &json!({"tool_name": "mcp__jev__jev_find", "tool_response": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]}),
        );
        assert_eq!(mcp.output.as_deref(), Some("a\nb"));
        let bash = parse(
            "PostToolUse",
            &json!({"tool_name": "Bash", "tool_response": {"stdout": "out", "stderr": "err", "interrupted": false}}),
        );
        assert_eq!(bash.output.as_deref(), Some("outerr"));
    }
}
