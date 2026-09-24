//! Muse command hooks (`PreToolUse` / `PostToolUse` groups in Muse settings).
//!
//! Muse uses Claude Code–compatible hook JSON (same `permission` / `tool_input`
//! shapes as Cursor `preToolUse`). Configure in `~/.config/muse/settings.json`.

use serde_json::Value;

use crate::adapters::cursor;

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

pub fn render(event_name: &str, outcome: &crate::policy::PolicyOutcome) -> Value {
    let normalized = match event_name {
        "PreToolUse" => "preToolUse",
        "PostToolUse" => "postToolUse",
        other => other,
    };
    cursor::render(normalized, outcome)
}
