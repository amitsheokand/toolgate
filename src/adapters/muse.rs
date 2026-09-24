//! Muse command hooks (`PreToolUse` / `PostToolUse` groups in Muse settings).
//!
//! Muse uses Claude Code–compatible hook JSON. Pre-tool replies must use
//! `hookSpecificOutput.permissionDecision` / `permissionDecisionReason` /
//! `updatedInput` (not Cursor's top-level `permission` fields).

use serde_json::{Value, json};

use crate::adapters::cursor;
use crate::policy::{Decision, PolicyOutcome};

pub type MuseReply = crate::adapters::cursor::CursorReply;

pub fn parse(event_name: &str, value: &Value) -> crate::event::ToolEvent {
    let normalized = match event_name {
        "PreToolUse" => "preToolUse",
        "PostToolUse" => "postToolUse",
        other => other,
    };
    let mut ev = cursor::parse(normalized, value);
    ev.harness = crate::event::Harness::Muse;
    ev
}

pub fn render(event_name: &str, outcome: &PolicyOutcome) -> Value {
    match event_name {
        "PreToolUse" => render_pre(outcome),
        "PostToolUse" => render_post(outcome),
        other => cursor::render(other, outcome),
    }
}

fn render_pre(outcome: &PolicyOutcome) -> Value {
    match &outcome.applied {
        Decision::Allow => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow"
            }
        }),
        Decision::Deny { agent_message, .. } => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": agent_message
            }
        }),
        Decision::Rewrite { updated_input, .. } => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "updatedInput": updated_input
            }
        }),
        Decision::ReplaceOutput { .. } => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow"
            }
        }),
    }
}

fn render_post(outcome: &PolicyOutcome) -> Value {
    match &outcome.applied {
        Decision::Allow => json!({}),
        Decision::Deny { .. } => json!({}),
        Decision::Rewrite { .. } => json!({}),
        Decision::ReplaceOutput { text, .. } => json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "updatedToolOutput": text
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Decision;

    #[test]
    fn pre_deny_uses_hook_specific_output() {
        let outcome = PolicyOutcome {
            raw: Decision::Deny {
                agent_message: "nope".into(),
                rule_id: "test".into(),
            },
            applied: Decision::Deny {
                agent_message: "nope".into(),
                rule_id: "test".into(),
            },
            rule_id: Some("test".into()),
        };
        let reply = render("PreToolUse", &outcome);
        assert_eq!(reply["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert!(reply.get("permission").is_none());
    }

    #[test]
    fn post_clip_uses_hook_specific_output() {
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
        let reply = render("PostToolUse", &outcome);
        assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert_eq!(reply["hookSpecificOutput"]["updatedToolOutput"], "clipped");
        assert!(reply.get("updated_mcp_tool_output").is_none());
    }
}
