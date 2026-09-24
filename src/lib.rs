//! toolgate library root: harness-side guardrails.
//!
//! one-grep narrows search; toolgate narrows what agents may *read*.
//! First gate: capped file reads (see [`read`]).

pub mod mcp;
pub mod read;
