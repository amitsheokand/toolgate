//! Golden harness fixtures: stdin payload → expected reply keys.

use std::path::PathBuf;

use serde_json::Value;
use toolgate::adapters;
use toolgate::event::Harness;
use toolgate::hook::{HookEnv, HookPaths, hook_stdio_with};
use toolgate::policy::{PolicyMode, decide, load_policy_file, resolve_mode};
use toolgate::telemetry::{append_row, row_from};

fn fixture_dir(harness: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(harness)
}

fn run_case(harness: Harness, event: &str, stem: &str, home: &PathBuf, cwd: Option<&str>) {
    let dir = fixture_dir(match harness {
        Harness::Cursor => "cursor",
        Harness::Muse => "muse",
        Harness::Opencode => "opencode",
        Harness::Pi => "pi",
    });
    let input_path = dir.join(format!("{stem}.in.json"));
    let golden_path = dir.join(format!("{stem}.reply.json"));
    let input = std::fs::read_to_string(&input_path).expect("read input");
    let golden: Value =
        serde_json::from_str(&std::fs::read_to_string(&golden_path).expect("read golden"))
            .expect("golden json");
    let mut payload: Value = serde_json::from_str(&input).expect("input json");
    if let Some(cwd) = cwd {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("cwd".into(), Value::String(cwd.to_string()));
        }
    }
    let env = HookEnv {
        paths: HookPaths::from_home(home),
        policy_override: None,
        telemetry_enabled: false,
    };
    let mut out = Vec::new();
    hook_stdio_with(
        harness,
        event,
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &mut out,
        &env,
    )
    .expect("hook");
    let reply: Value =
        serde_json::from_str(String::from_utf8(out).unwrap().trim()).expect("reply json");
    assert_eq!(reply, golden, "{stem}");
}

#[test]
fn cursor_read_over_limit_denies() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let body = (1..=500)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(tmp.path().join("big.txt"), body).expect("write");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".config/toolgate")).expect("cfg");
    run_case(
        Harness::Cursor,
        "preToolUse",
        "read_over_limit",
        &home,
        Some(tmp.path().to_str().unwrap()),
    );
}

#[test]
fn cursor_bounded_read_allows() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".config/toolgate")).expect("cfg");
    run_case(Harness::Cursor, "preToolUse", "bounded_read", &home, None);
}

#[test]
fn cursor_small_read_allows() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".config/toolgate")).expect("cfg");
    run_case(Harness::Cursor, "preToolUse", "small_read", &home, None);
}

#[test]
fn cursor_shell_deny() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".config/toolgate")).expect("cfg");
    run_case(Harness::Cursor, "preToolUse", "shell_deny", &home, None);
}

#[test]
fn cursor_unknown_allows() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".config/toolgate")).expect("cfg");
    run_case(
        Harness::Cursor,
        "preToolUse",
        "unknown_payload",
        &home,
        None,
    );
}

#[test]
fn observe_mode_logs_but_allows() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".config/toolgate")).expect("cfg");
    let policy_path = home.join(".config/toolgate/policy.toml");
    std::fs::write(&policy_path, "mode = \"observe\"\n").expect("policy");
    let mut policy = load_policy_file(&policy_path);
    policy.mode = resolve_mode(&policy, None);
    assert_eq!(policy.mode, PolicyMode::Observe);
    let input: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_dir("cursor").join("shell_deny.in.json")).unwrap(),
    )
    .unwrap();
    let event = adapters::parse_event(Harness::Cursor, "preToolUse", &input);
    let outcome = decide(&event, &policy, None);
    assert!(matches!(
        outcome.raw,
        toolgate::policy::Decision::Deny { .. }
    ));
    assert_eq!(outcome.applied, toolgate::policy::Decision::Allow);
    let telemetry_path = home.join(".local/state/toolgate/events.jsonl");
    let row = row_from(&event, &outcome, policy.mode);
    append_row(&telemetry_path, &row);
    let log = std::fs::read_to_string(&telemetry_path).expect("log");
    assert!(!log.contains("rm -rf"));
    assert!(log.contains("deny"));
}
