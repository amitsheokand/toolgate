//! Append-only JSONL telemetry (digests only, no raw secrets).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::event::{Phase, ToolEvent};
use crate::policy::{Decision, PolicyMode, PolicyOutcome};

/// One JSONL row.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryRow {
    pub ts: u64,
    pub harness: String,
    pub session_id: String,
    pub phase: String,
    pub tool: String,
    pub path_digest: Option<String>,
    pub cmd_digest: Option<String>,
    pub input_chars: u64,
    pub output_chars: u64,
    pub decision: String,
    pub rule_id: Option<String>,
    pub mode: String,
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn tool_label(event: &ToolEvent) -> String {
    match &event.tool {
        crate::event::ToolKind::Mcp { server, tool } => format!("mcp:{server}:{tool}"),
        crate::event::ToolKind::Other(n) => format!("other:{n}"),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "other".into()),
    }
}

fn decision_label(d: &Decision) -> String {
    match d {
        Decision::Allow => "allow".into(),
        Decision::Deny { .. } => "deny".into(),
        Decision::Rewrite { .. } => "rewrite".into(),
        Decision::ReplaceOutput { .. } => "replace_output".into(),
    }
}

/// Build a row from an event and policy outcome (no raw path/command text).
#[must_use]
pub fn row_from(event: &ToolEvent, outcome: &PolicyOutcome, mode: PolicyMode) -> TelemetryRow {
    let path_digest = event.args.path.as_deref().map(digest);
    let cmd_digest = event.args.command.as_deref().map(digest);
    let input_chars = event
        .args
        .raw_input
        .as_ref()
        .map(|v| {
            serde_json::to_string(v)
                .map(|s| s.len() as u64)
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let output_chars = event.output.as_deref().map(|s| s.len() as u64).unwrap_or(0);
    TelemetryRow {
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        harness: serde_json::to_value(event.harness)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".into()),
        session_id: event.session_id.clone(),
        phase: match event.phase {
            Phase::Pre => "pre".into(),
            Phase::Post => "post".into(),
        },
        tool: tool_label(event),
        path_digest,
        cmd_digest,
        input_chars,
        output_chars,
        decision: decision_label(&outcome.raw),
        rule_id: outcome.rule_id.clone(),
        mode: match mode {
            PolicyMode::Enforce => "enforce".into(),
            PolicyMode::Observe => "observe".into(),
        },
    }
}

/// Append one row; failures are ignored (fail open).
pub fn append_row(path: &Path, row: &TelemetryRow) {
    if let Ok(line) = serde_json::to_string(row) {
        let _ = append_line_locked(path, &line);
    }
}

fn append_line_locked(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        unsafe {
            libc::flock(fd, libc::LOCK_EX);
        }
    }
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Harness, NormalizedArgs, ToolEvent, ToolKind};
    use crate::policy::{Decision, PolicyOutcome};

    #[test]
    fn row_has_no_raw_path() {
        let event = ToolEvent {
            harness: Harness::Cursor,
            phase: Phase::Pre,
            tool: ToolKind::Read,
            args: NormalizedArgs {
                path: Some("/secret/path.txt".into()),
                ..Default::default()
            },
            output: None,
            session_id: "abc".into(),
            cwd: "/tmp".into(),
        };
        let outcome = PolicyOutcome {
            raw: Decision::Allow,
            applied: Decision::Allow,
            rule_id: None,
        };
        let row = row_from(&event, &outcome, PolicyMode::Enforce);
        let json = serde_json::to_string(&row).expect("json");
        assert!(!json.contains("/secret"));
        assert!(json.contains("path_digest"));
    }
}
