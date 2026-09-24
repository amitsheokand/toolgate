//! toolgate library root: harness-side guardrails.
//!
//! one-grep narrows search; toolgate narrows what agents may *read*.
//! First gate: capped file reads (see [`read`]).

pub mod edit;
pub mod gate;
pub mod mcp;
pub mod read;
pub mod run;
