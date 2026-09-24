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
        "PostToolUse" => cursor::render("postToolUse", outcome),
        other => cursor::render(other, outcome),
    }
}

fn render_pre(outcome: &PolicyOutcome) -> Value {
    match &outcome.applied {
        Decision::Allow => json!({
            "hookSpecificOutput": { "permissionDecision": "allow" }
        }),
        Decision::Deny { agent_message, .. } => json!({
            "hookSpecificOutput": {
                "permissionDecision": "deny",
                "permissionDecisionReason": agent_message
            }
        }),
        Decision::Rewrite { updated_input, .. } => json!({
            "hookSpecificOutput": {
                "permissionDecision": "allow",
                "updatedInput": updated_input
            }
        }),
        Decision::ReplaceOutput { .. } => json!({
            "hookSpecificOutput": { "permissionDecision": "allow" }
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
        assert!(reply.get("permission").is_none());
    }
}
