//! toolgate library root: harness-side guardrails.
//!
//! one-grep narrows search; toolgate narrows what agents may *read*, *edit*,
//! and *run*. First gates: capped file reads (see [`read`]).

pub mod adapters;
pub mod edit;
pub mod event;
pub mod gate;
pub mod hook;
pub mod policy;
pub mod read;
pub mod run;
pub mod telemetry;

#[cfg(feature = "server")]
pub mod mcp;
