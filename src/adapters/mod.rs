//! Harness adapters: JSON payloads ↔ [`ToolEvent`] ↔ policy decisions.
//!
//! See per-harness module docs for payload sources.

mod cursor;
mod muse;
mod opencode;
mod pi;

use serde_json::Value;

use crate::event::{Harness, ToolEvent};
use crate::policy::{Decision, PolicyOutcome};

pub use cursor::CursorReply;
pub use muse::MuseReply;
pub use opencode::OpencodeReply;
pub use pi::PiReply;

/// Parse stdin JSON into a normalized event (`unknown` shapes → allow via `Other`).
#[must_use]
pub fn parse_event(harness: Harness, event_name: &str, value: &Value) -> ToolEvent {
    match harness {
        Harness::Cursor => cursor::parse(event_name, value),
        Harness::Muse => muse::parse(event_name, value),
        Harness::Opencode => opencode::parse(event_name, value),
        Harness::Pi => pi::parse(event_name, value),
    }
}

/// Render harness-specific reply JSON from an applied decision.
#[must_use]
pub fn render_reply(harness: Harness, event_name: &str, outcome: &PolicyOutcome) -> Value {
    match harness {
        Harness::Cursor => cursor::render(event_name, outcome),
        Harness::Muse => muse::render(event_name, outcome),
        Harness::Opencode => opencode::render(event_name, outcome),
        Harness::Pi => pi::render(event_name, outcome),
    }
}

/// Fail-open allow reply per harness.
#[must_use]
pub fn allow_reply(harness: Harness, event_name: &str) -> Value {
    render_reply(
        harness,
        event_name,
        &PolicyOutcome {
            raw: Decision::Allow,
            applied: Decision::Allow,
            rule_id: None,
        },
    )
}
