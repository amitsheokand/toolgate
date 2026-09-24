//! Cursor Agent hooks (`preToolUse` / `postToolUse` / `afterShellExecution` / `afterMCPExecution`).
//!
//! Schema: <https://cursor.com/docs/agent/hooks>
//! - `preToolUse`: deny / rewrite via `permission` + `updated_input`
//! - `postToolUse`: MCP output replace via `updated_mcp_tool_output` only
//! - `afterShellExecution` / `afterMCPExecution`: telemetry only (no model output change)

use serde_json::{Map, Value, json};

use crate::event::{Harness, NormalizedArgs, Phase, ToolEvent, ToolKind};
use crate::policy::{Decision, PolicyOutcome};

/// Cursor hook reply (union of hook output shapes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorReply {
    PreToolUse {
        permission: String,
        agent_message: Option<String>,
        updated_input: Option<Value>,
    },
    PostToolUse {
        updated_mcp_tool_output: Option<Value>,
        additional_context: Option<String>,
    },
    Empty,
}

pub fn parse(event_name: &str, value: &Value) -> ToolEvent {
    let phase = if event_name.starts_with("post") || event_name.starts_with("after") {
        Phase::Post
    } else {
        Phase::Pre
    };
    let hook_event = value.get("hook_event_name").and_then(Value::as_str);
    let tool_name = value.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let classify = classify_name(hook_event, tool_name);
    let tool = map_tool_name(classify, value);
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("workspace_roots")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
        })
        .unwrap_or(".")
        .to_owned();
    let session_id = value
        .get("conversation_id")
        .or_else(|| value.get("tool_use_id"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let args = normalize_args(value, &tool);
    let output = post_output(event_name, value);
    ToolEvent {
        harness: Harness::Cursor,
        phase,
        tool,
        args,
        output,
        session_id,
        cwd,
    }
}

fn post_output(event_name: &str, value: &Value) -> Option<String> {
    match event_name {
        "postToolUse" => value
            .get("tool_output")
            .and_then(Value::as_str)
            .map(str::to_owned),
        "afterShellExecution" => value
            .get("output")
            .and_then(Value::as_str)
            .map(str::to_owned),
        "afterMCPExecution" => value
            .get("result_json")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

/// Cursor always sends `hook_event_name` alongside `tool_name`; only dedicated
/// after/before MCP/shell hooks use the event name for classification.
fn classify_name<'a>(hook_event: Option<&'a str>, tool_name: &'a str) -> &'a str {
    match hook_event {
        Some("beforeMCPExecution" | "afterMCPExecution" | "afterShellExecution") => {
            hook_event.unwrap()
        }
        _ => tool_name,
    }
}

fn mcp_kind(value: &Value, tool: &str) -> ToolKind {
    let server = value
        .get("mcp_server_name")
        .and_then(Value::as_str)
        .unwrap_or("mcp")
        .to_owned();
    ToolKind::Mcp {
        server,
        tool: tool.to_owned(),
    }
}

fn map_tool_name(name: &str, value: &Value) -> ToolKind {
    if value.get("mcp_server_name").is_some()
        && !matches!(
            name,
            "beforeMCPExecution" | "afterMCPExecution" | "afterShellExecution"
        )
    {
        return mcp_kind(value, name);
    }
    match name {
        "Read" | "beforeReadFile" => ToolKind::Read,
        "Write" | "afterFileEdit" => ToolKind::Write,
        "Edit" => ToolKind::Edit,
        "Shell" | "Bash" | "beforeShellExecution" | "afterShellExecution" => ToolKind::Shell,
        "Grep" => ToolKind::Grep,
        "Glob" => ToolKind::Glob,
        other if other.starts_with("MCP:") => {
            let rest = other.strip_prefix("MCP:").unwrap_or(other);
            let (_server, tool) = rest.split_once(':').unwrap_or(("mcp", rest));
            mcp_kind(value, tool)
        }
        "beforeMCPExecution" | "afterMCPExecution" => {
            let tool = value
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_owned();
            mcp_kind(value, &tool)
        }
        _ => ToolKind::Other(name.to_owned()),
    }
}

fn normalize_args(value: &Value, tool: &ToolKind) -> NormalizedArgs {
    let mut args = NormalizedArgs::default();
    if let Some(cmd) = value.get("command").and_then(Value::as_str) {
        args.command = Some(cmd.to_owned());
    }
    let input = value
        .get("tool_input")
        .cloned()
        .or_else(|| value.get("tool_input").cloned());
    if let Some(obj) = input.and_then(|v| v.as_object().cloned()) {
        args.raw_input = Some(Value::Object(obj.clone()));
        args.path = string_field(&obj, &["path", "file_path", "target_file"]);
        args.offset = obj.get("offset").and_then(|v| v.as_u64());
        args.limit = obj.get("limit").and_then(|v| v.as_u64());
        args.pattern = string_field(&obj, &["pattern", "glob_pattern", "glob"]);
        if args.command.is_none() {
            args.command = string_field(&obj, &["command"]);
        }
    }
    if args.path.is_none() {
        args.path = value
            .get("file_path")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    if matches!(tool, ToolKind::Shell) && args.command.is_none() {
        args.command = value
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    args
}

fn string_field(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(Value::String(s)) = obj.get(*key) {
            return Some(s.clone());
        }
    }
    None
}

pub fn render(event_name: &str, outcome: &PolicyOutcome) -> Value {
    match &outcome.applied {
        Decision::Allow => {
            if event_name == "preToolUse" || event_name == "beforeShellExecution" {
                json!({ "permission": "allow" })
            } else {
                json!({})
            }
        }
        Decision::Deny { agent_message, .. } => {
            if event_name == "preToolUse" || event_name == "beforeShellExecution" {
                json!({
                    "permission": "deny",
                    "agent_message": agent_message
                })
            } else {
                json!({})
            }
        }
        Decision::Rewrite { updated_input, .. } => json!({
            "permission": "allow",
            "updated_input": updated_input
        }),
        Decision::ReplaceOutput { text, .. } => post_replace(event_name, &text),
    }
}

fn post_replace(event_name: &str, text: &str) -> Value {
    match event_name {
        "postToolUse" => json!({
            "updated_mcp_tool_output": { "content": [{ "type": "text", "text": text }] }
        }),
        "afterShellExecution" | "afterMCPExecution" => json!({}),
        _ => json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{Decision, PolicyOutcome};

    #[test]
    fn after_mcp_execution_classifies_as_mcp() {
        let payload = json!({
            "hook_event_name": "afterMCPExecution",
            "tool_name": "search_ranked",
            "mcp_server_name": "one-grep",
            "result_json": "x".repeat(5000)
        });
        let ev = parse("afterMCPExecution", &payload);
        assert!(matches!(ev.tool, ToolKind::Mcp { .. }));
    }

    #[test]
    fn after_shell_clip_does_not_replace_output() {
        let outcome = PolicyOutcome {
            raw: Decision::ReplaceOutput {
                text: "clipped".into(),
                rule_id: "clip.output".into(),
            },
            applied: Decision::ReplaceOutput {
                text: "clipped".into(),
                rule_id: "clip.output".into(),
            },
            rule_id: Some("clip.output".into()),
        };
        let reply = render("afterShellExecution", &outcome);
        assert_eq!(reply, json!({}));
    }

    #[test]
    fn muse_bash_maps_to_shell() {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "rm -rf /tmp/x" }
        });
        let ev = parse("preToolUse", &payload);
        assert_eq!(ev.tool, ToolKind::Shell);
    }

    #[test]
    fn pre_tool_use_ignores_hook_event_name_for_classification() {
        let payload = json!({
            "hook_event_name": "preToolUse",
            "tool_name": "Shell",
            "tool_input": { "command": "rm -rf /tmp/x" },
            "cwd": "/tmp"
        });
        let ev = parse("preToolUse", &payload);
        assert_eq!(ev.tool, ToolKind::Shell);
    }

    #[test]
    fn post_mcp_uses_tool_name_when_server_present() {
        let payload = json!({
            "hook_event_name": "postToolUse",
            "tool_name": "search",
            "mcp_server_name": "one-grep",
            "tool_output": "x".repeat(5000),
            "cwd": "/tmp"
        });
        let ev = parse("postToolUse", &payload);
        assert!(matches!(
            ev.tool,
            ToolKind::Mcp {
                server,
                tool
            } if server == "one-grep" && tool == "search"
        ));
    }
}
