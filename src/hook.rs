//! Harness hook driver: stdin JSON → policy → stdout JSON (fail open).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters;
use crate::event::Harness;
use crate::policy::{decide, load_policy_file, resolve_mode};
use crate::telemetry::{append_row, row_from};

/// Runtime paths (injectable in tests; defaults use XDG-style dirs under home).
#[derive(Debug, Clone)]
pub struct HookPaths {
    pub policy_file: PathBuf,
    pub telemetry_file: PathBuf,
    pub archive_dir: PathBuf,
}

impl HookPaths {
    /// Default locations under `$HOME/.config` and `$HOME/.local/state`.
    #[must_use]
    pub fn from_home(home: &Path) -> Self {
        Self {
            policy_file: home.join(".config/toolgate/policy.toml"),
            telemetry_file: home.join(".local/state/toolgate/events.jsonl"),
            archive_dir: home.join(".local/state/toolgate/archive"),
        }
    }
}

/// Hook environment (no process env reads when fields are set).
#[derive(Debug, Clone)]
pub struct HookEnv {
    pub paths: HookPaths,
    pub policy_override: Option<String>,
    pub telemetry_enabled: bool,
}

impl HookEnv {
    #[must_use]
    pub fn from_process() -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        Self {
            paths: HookPaths::from_home(&home),
            policy_override: std::env::var("TOOLGATE_POLICY").ok(),
            telemetry_enabled: std::env::var("TOOLGATE_TELEMETRY").ok().as_deref() != Some("0"),
        }
    }
}

/// Legacy Cursor read hook types (re-exported for tests).
pub use legacy::{PreToolUseInput, PreToolUseOutput, allow_unchanged, decide_cursor_read};

mod legacy {
    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value};

    use crate::read::WHOLE_FILE_LIMIT_LINES;

    #[derive(Debug, Deserialize)]
    pub struct PreToolUseInput {
        pub tool_name: Option<String>,
        pub tool_input: Option<Value>,
        pub cwd: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, PartialEq, Eq)]
    pub struct PreToolUseOutput {
        pub permission: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub user_message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub agent_message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub updated_input: Option<Value>,
    }

    #[must_use]
    pub fn allow_unchanged() -> PreToolUseOutput {
        PreToolUseOutput {
            permission: "allow".into(),
            user_message: None,
            agent_message: None,
            updated_input: None,
        }
    }

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
        if obj.contains_key("offset") || obj.contains_key("limit") {
            return allow_unchanged();
        }
        let path = file_path_from_input(obj);
        let cwd = input
            .cwd
            .as_deref()
            .map(std::path::Path::new)
            .unwrap_or_else(|| std::path::Path::new("."));
        let (abs, display) = match path {
            Some(p) => (resolve_read_path(cwd, p), p.to_owned()),
            None => return allow_unchanged(),
        };
        let lines = crate::read::count_file_lines(&abs);
        if lines.is_none() || lines.is_some_and(|n| n <= WHOLE_FILE_LIMIT_LINES) {
            return allow_unchanged();
        }
        let total = lines.expect("checked");
        PreToolUseOutput {
            permission: "deny".into(),
            user_message: None,
            agent_message: Some(format!(
                "{display} has {total} lines; whole-file reads over 400 lines are capped. \
                 Re-read with offset/limit for the range you need (e.g. the path:start-end from \
                 search), or call one-grep `context` for the enclosing symbol."
            )),
            updated_input: None,
        }
    }

    fn file_path_from_input(obj: &Map<String, Value>) -> Option<&str> {
        for key in ["path", "file_path", "target_file"] {
            if let Some(Value::String(s)) = obj.get(key) {
                return Some(s.as_str());
            }
        }
        None
    }

    fn resolve_read_path(cwd: &std::path::Path, raw: &str) -> std::path::PathBuf {
        if std::path::Path::new(raw).is_absolute() {
            std::path::PathBuf::from(raw)
        } else {
            cwd.join(raw)
        }
    }
}

fn emit_json<W: Write>(writer: &mut W, value: &Value) -> std::io::Result<()> {
    match serde_json::to_string(value) {
        Ok(json) => writeln!(writer, "{json}"),
        Err(_) => writeln!(writer, r#"{{"permission":"allow"}}"#),
    }
}

/// Run a harness hook with injected I/O.
pub fn hook_stdio_with<R, W>(
    harness: Harness,
    event_name: &str,
    mut reader: R,
    mut writer: W,
    env: &HookEnv,
) -> std::io::Result<()>
where
    R: Read,
    W: Write,
{
    let mut buf = String::new();
    let reply = match reader.read_to_string(&mut buf) {
        Err(_) => adapters::allow_reply(harness, event_name),
        Ok(_) => match serde_json::from_str::<Value>(&buf) {
            Err(_) => adapters::allow_reply(harness, event_name),
            Ok(value) => run_hook(harness, event_name, &value, env),
        },
    };
    emit_json(&mut writer, &reply)
}

fn run_hook(harness: Harness, event_name: &str, value: &Value, env: &HookEnv) -> Value {
    let mut policy = load_policy_file(&env.paths.policy_file);
    policy.mode = resolve_mode(&policy, env.policy_override.as_deref());
    let event = adapters::parse_event(harness, event_name, value);
    let outcome = decide(&event, &policy, Some(&env.paths.archive_dir));
    if env.telemetry_enabled {
        let row = row_from(&event, &outcome, policy.mode);
        append_row(&env.paths.telemetry_file, &row);
    }
    adapters::render_reply(harness, event_name, &outcome)
}

/// Read stdin, run policy hook, print one JSON object (exit 0).
pub fn hook_stdio(harness: Harness, event_name: &str) -> std::io::Result<()> {
    let env = HookEnv::from_process();
    hook_stdio_with(
        harness,
        event_name,
        std::io::stdin(),
        std::io::stdout(),
        &env,
    )
}

/// Legacy cursor-read entry (delegates to unified hook).
pub fn cursor_read_stdio() -> std::io::Result<()> {
    hook_stdio(Harness::Cursor, "preToolUse")
}

/// Testable legacy cursor-read with env override.
pub fn cursor_read_stdio_with<R, W>(
    reader: R,
    mut writer: W,
    read_hook_env: Option<&str>,
) -> std::io::Result<()>
where
    R: Read,
    W: Write,
{
    if read_hook_env == Some("0") {
        return emit_json(
            &mut writer,
            &adapters::allow_reply(Harness::Cursor, "preToolUse"),
        );
    }
    let env = HookEnv::from_process();
    hook_stdio_with(Harness::Cursor, "preToolUse", reader, writer, &env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hook_allows_small_file_via_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "one\n").expect("write");
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".config/toolgate")).expect("cfg");
        let env = HookEnv {
            paths: HookPaths::from_home(&home),
            policy_override: None,
            telemetry_enabled: false,
        };
        let cwd = dir.path().to_string_lossy();
        let input =
            format!(r#"{{"tool_name":"Read","tool_input":{{"path":"a.txt"}},"cwd":"{cwd}"}}"#);
        let mut out = Vec::new();
        hook_stdio_with(
            Harness::Cursor,
            "preToolUse",
            input.as_bytes(),
            &mut out,
            &env,
        )
        .expect("stdio");
        let parsed: Value = serde_json::from_str(String::from_utf8(out).unwrap().trim()).unwrap();
        assert_eq!(parsed["permission"], "allow");
    }

    #[test]
    fn hook_denies_large_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = (1..=500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), body).expect("write");
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".config/toolgate")).expect("cfg");
        let env = HookEnv {
            paths: HookPaths::from_home(&home),
            policy_override: None,
            telemetry_enabled: false,
        };
        let cwd = dir.path().to_string_lossy();
        let input =
            format!(r#"{{"tool_name":"Read","tool_input":{{"path":"big.txt"}},"cwd":"{cwd}"}}"#);
        let mut out = Vec::new();
        hook_stdio_with(
            Harness::Cursor,
            "preToolUse",
            input.as_bytes(),
            &mut out,
            &env,
        )
        .expect("stdio");
        let parsed: Value = serde_json::from_str(String::from_utf8(out).unwrap().trim()).unwrap();
        assert_eq!(parsed["permission"], "deny");
    }

    #[test]
    fn bad_json_fail_open() {
        let home = tempfile::tempdir().expect("tempdir");
        let env = HookEnv {
            paths: HookPaths::from_home(home.path()),
            policy_override: None,
            telemetry_enabled: false,
        };
        let mut out = Vec::new();
        hook_stdio_with(
            Harness::Cursor,
            "preToolUse",
            &b"not-json"[..],
            &mut out,
            &env,
        )
        .expect("stdio");
        let parsed: Value = serde_json::from_str(String::from_utf8(out).unwrap().trim()).unwrap();
        assert_eq!(parsed["permission"], "allow");
    }

    #[test]
    fn legacy_decide_cursor_read_still_works() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = (1..=500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), body).expect("write");
        let input = legacy::PreToolUseInput {
            tool_name: Some("Read".into()),
            tool_input: Some(json!({ "path": "big.txt" })),
            cwd: Some(dir.path().to_string_lossy().into_owned()),
        };
        let out = legacy::decide_cursor_read(&input, None);
        assert_eq!(out.permission, "deny");
    }
}
