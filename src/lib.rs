//! toolgate library root: harness-side guardrails.
//!
//! one-grep narrows search; toolgate narrows what agents may *read*, *edit*,
//! and *run*. First gates: capped file reads (see [`read`]).

pub mod edit;
pub mod gate;
pub mod hook;
pub mod read;
pub mod run;

#[cfg(feature = "server")]
pub mod mcp;
