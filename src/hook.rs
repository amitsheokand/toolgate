//! Cursor Agent hooks (stdio JSON in/out).

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::read::WHOLE_FILE_LIMIT_LINES;

/// Whole-file read cap injected by the cursor-read hook.
const HOOK_READ_LIMIT: u64 = 400;

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
#[derive(Debug, Serialize, PartialEq, Eq)]
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

/// Decide the cursor-read hook response for a canned `preToolUse` payload.
#[must_use]
pub fn decide_cursor_read(input: &PreToolUseInput) -> PreToolUseOutput {
    if std::env::var("TOOLGATE_READ_HOOK").as_deref() == Ok("0") {
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
    let abs = match path {
        Some(p) => resolve_read_path(cwd, p),
        None => return allow_unchanged(),
    };
    let lines = count_file_lines(&abs);
    if lines.is_none() || lines.is_some_and(|n| n <= WHOLE_FILE_LIMIT_LINES) {
        return allow_unchanged();
    }
    let _total = lines.expect("checked");
    let mut updated = obj.clone();
    updated.insert("limit".into(), Value::Number(HOOK_READ_LIMIT.into()));
    PreToolUseOutput {
        permission: "allow".into(),
        user_message: None,
        agent_message: None,
        updated_input: Some(Value::Object(updated)),
    }
    // Docs: agent_message is only delivered on deny; we use allow+updated_input
    // so the capped read still runs. Message intent is in the receipt.
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

/// Read stdin, run [`decide_cursor_read`], print one JSON object (exit 0).
///
/// # Errors
///
/// I/O failures reading stdin or writing stdout.
pub fn cursor_read_stdio() -> std::io::Result<()> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let input: PreToolUseInput = serde_json::from_str(&buf).unwrap_or(PreToolUseInput {
        tool_name: None,
        tool_input: None,
        cwd: None,
    });
    let out = decide_cursor_read(&input);
    println!("{}", serde_json::to_string(&out).expect("serialize"));
    Ok(())
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
        let out = decide_cursor_read(&payload(
            json!({ "path": "a.txt" }),
            dir.path().to_str().unwrap(),
        ));
        assert_eq!(out, allow_unchanged());
    }

    #[test]
    fn hook_limits_large_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = (1..=500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), body).expect("write");
        let out = decide_cursor_read(&payload(
            json!({ "path": "big.txt" }),
            dir.path().to_str().unwrap(),
        ));
        assert_eq!(out.permission, "allow");
        let updated = out.updated_input.expect("updated");
        assert_eq!(updated["limit"], 400);
        assert!(out.agent_message.is_none());
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
            let out = decide_cursor_read(&payload(tool_input, cwd));
            assert_eq!(out, allow_unchanged());
        }
    }

    #[test]
    fn hook_allows_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = decide_cursor_read(&payload(
            json!({ "path": "nope.txt" }),
            dir.path().to_str().unwrap(),
        ));
        assert_eq!(out, allow_unchanged());
    }

    #[test]
    fn hook_allows_unknown_shape() {
        let out = decide_cursor_read(&payload(json!({ "foo": 1 }), "/tmp"));
        assert_eq!(out, allow_unchanged());
        let out = decide_cursor_read(&PreToolUseInput {
            tool_name: None,
            tool_input: None,
            cwd: None,
        });
        assert_eq!(out, allow_unchanged());
    }

    #[test]
    fn hook_env_off_always_allows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = (1..=500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), body).expect("write");
        unsafe {
            std::env::set_var("TOOLGATE_READ_HOOK", "0");
        }
        let out = decide_cursor_read(&payload(
            json!({ "path": "big.txt" }),
            dir.path().to_str().unwrap(),
        ));
        unsafe {
            std::env::remove_var("TOOLGATE_READ_HOOK");
        }
        assert_eq!(out, allow_unchanged());
    }

    #[test]
    fn hook_accepts_alternate_path_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = (1..=500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), body).expect("write");
        let cwd = dir.path().to_str().unwrap();
        for key in ["file_path", "target_file"] {
            let out = decide_cursor_read(&payload(json!({ key: "big.txt" }), cwd));
            assert_eq!(
                out.updated_input.as_ref().and_then(|v| v.get("limit")),
                Some(&json!(400))
            );
        }
    }
}
