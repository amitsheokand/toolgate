//! Cursor Agent hooks (stdio JSON in/out).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::read::WHOLE_FILE_LIMIT_LINES;

/// `preToolUse` hook input (subset used by cursor-read).
#[derive(Debug, Deserialize)]
pub struct PreToolUseInput {
    /// Tool name (e.g. `Read`).
    pub tool_name: Option<String>,
    /// Tool arguments.
    pub tool_input: Option<Value>,
    /// Agent working directory.
    pub cwd: Option<String>,
}

/// `preToolUse` hook output.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PreToolUseOutput {
    /// `allow` or `deny`.
    pub permission: String,
    /// Shown to the user when denied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_message: Option<String>,
    /// Fed back to the agent when denied (per Cursor docs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_message: Option<String>,
    /// Modified tool input when allowing with changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
}

/// Always-allow response (fail-open default).
#[must_use]
pub fn allow_unchanged() -> PreToolUseOutput {
    PreToolUseOutput {
        permission: "allow".into(),
        user_message: None,
        agent_message: None,
        updated_input: None,
    }
}

fn deny_large_read(display_path: &str, lines: u64) -> PreToolUseOutput {
    PreToolUseOutput {
        permission: "deny".into(),
        user_message: None,
        agent_message: Some(format!(
            "{display_path} has {lines} lines; whole-file reads over 400 lines are capped. \
             Re-read with offset/limit for the range you need (e.g. the path:start-end from \
             search), or call one-grep `context` for the enclosing symbol."
        )),
        updated_input: None,
    }
}

/// Decide the cursor-read hook response for a canned `preToolUse` payload.
///
/// `read_hook_env` is the value of `TOOLGATE_READ_HOOK` when set (e.g. `"0"` disables).
#[must_use]
pub fn decide_cursor_read(
    input: &PreToolUseInput,
    read_hook_env: Option<&str>,
) -> PreToolUseOutput {
    if read_hook_env == Some("0") {
        return allow_unchanged();
    }
    let tool_input = input.tool_input.as_ref();
    if tool_input.is_none() {
        return allow_unchanged();
    }
    let obj = tool_input.and_then(Value::as_object);
    if obj.is_none() {
        return allow_unchanged();
    }
    let obj = obj.expect("checked");
    if has_offset_or_limit(obj) {
        return allow_unchanged();
    }
    let path = file_path_from_input(obj);
    let cwd = input
        .cwd
        .as_deref()
        .map(Path::new)
        .unwrap_or_else(|| Path::new("."));
    let (abs, display) = match path {
        Some(p) => (resolve_read_path(cwd, p), p.to_owned()),
        None => return allow_unchanged(),
    };
    let lines = count_file_lines(&abs);
    if lines.is_none() || lines.is_some_and(|n| n <= WHOLE_FILE_LIMIT_LINES) {
        return allow_unchanged();
    }
    let total = lines.expect("checked");
    deny_large_read(&display, total)
}

fn has_offset_or_limit(obj: &Map<String, Value>) -> bool {
    obj.contains_key("offset") || obj.contains_key("limit")
}

fn file_path_from_input(obj: &Map<String, Value>) -> Option<&str> {
    for key in ["path", "file_path", "target_file"] {
        if let Some(Value::String(s)) = obj.get(key) {
            return Some(s.as_str());
        }
    }
    None
}

fn resolve_read_path(cwd: &Path, raw: &str) -> PathBuf {
    if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        cwd.join(raw)
    }
}

fn count_file_lines(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    if text.is_empty() {
        return Some(0);
    }
    Some(text.lines().count() as u64)
}

/// Serialize hook output; on failure returns minimal allow JSON.
fn emit_output<W: Write>(writer: &mut W, out: &PreToolUseOutput) -> std::io::Result<()> {
    match serde_json::to_string(out) {
        Ok(json) => writeln!(writer, "{json}"),
        Err(_) => writeln!(writer, r#"{{"permission":"allow"}}"#),
    }
}

/// Run cursor-read hook with injected reader/writer (testable).
pub fn cursor_read_stdio_with<R, W>(
    mut reader: R,
    mut writer: W,
    read_hook_env: Option<&str>,
) -> std::io::Result<()>
where
    R: Read,
    W: Write,
{
    let mut buf = String::new();
    let out = match reader.read_to_string(&mut buf) {
        Err(_) => allow_unchanged(),
        Ok(_) => match serde_json::from_str::<PreToolUseInput>(&buf) {
            Ok(input) => decide_cursor_read(&input, read_hook_env),
            Err(_) => allow_unchanged(),
        },
    };
    emit_output(&mut writer, &out)
}

/// Read stdin, run [`decide_cursor_read`], print one JSON object (exit 0).
pub fn cursor_read_stdio() -> std::io::Result<()> {
    let env = std::env::var("TOOLGATE_READ_HOOK").ok();
    cursor_read_stdio_with(std::io::stdin(), std::io::stdout(), env.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(tool_input: Value, cwd: &str) -> PreToolUseInput {
        PreToolUseInput {
            tool_name: Some("Read".into()),
            tool_input: Some(tool_input),
            cwd: Some(cwd.into()),
        }
    }

    #[test]
    fn hook_allows_small_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "one\n").expect("write");
        let out = decide_cursor_read(
            &payload(json!({ "path": "a.txt" }), dir.path().to_str().unwrap()),
            None,
        );
        assert_eq!(out, allow_unchanged());
    }

    #[test]
    fn hook_denies_large_file_with_line_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = (1..=500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), body).expect("write");
        let out = decide_cursor_read(
            &payload(json!({ "path": "big.txt" }), dir.path().to_str().unwrap()),
            None,
        );
        assert_eq!(out.permission, "deny");
        let msg = out.agent_message.expect("message");
        assert!(msg.contains("big.txt"));
        assert!(msg.contains("500 lines"));
        assert!(msg.contains("offset/limit"));
    }

    #[test]
    fn hook_allows_when_offset_or_limit_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = (1..=500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), body).expect("write");
        let cwd = dir.path().to_str().unwrap();
        for tool_input in [
            json!({ "path": "big.txt", "offset": 10 }),
            json!({ "path": "big.txt", "limit": 50 }),
        ] {
            let out = decide_cursor_read(&payload(tool_input, cwd), None);
            assert_eq!(out, allow_unchanged());
        }
    }

    #[test]
    fn hook_env_off_always_allows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = (1..=500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), body).expect("write");
        let out = decide_cursor_read(
            &payload(json!({ "path": "big.txt" }), dir.path().to_str().unwrap()),
            Some("0"),
        );
        assert_eq!(out, allow_unchanged());
    }

    #[test]
    fn cursor_read_stdio_allows_on_bad_json() {
        let mut out = Vec::new();
        cursor_read_stdio_with(&b"not json"[..], &mut out, None).expect("stdio");
        assert_eq!(
            String::from_utf8(out).unwrap().trim(),
            r#"{"permission":"allow"}"#
        );
    }

    #[test]
    fn cursor_read_stdio_emits_deny_for_large_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = (1..=500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), body).expect("write");
        let cwd = dir.path().to_string_lossy();
        let input =
            format!(r#"{{"tool_name":"Read","tool_input":{{"path":"big.txt"}},"cwd":"{cwd}"}}"#);
        let mut out = Vec::new();
        cursor_read_stdio_with(input.as_bytes(), &mut out, None).expect("stdio");
        let body = String::from_utf8(out).expect("utf8");
        let parsed: Value = serde_json::from_str(body.trim()).expect("json");
        assert_eq!(parsed["permission"], "deny");
    }
}
