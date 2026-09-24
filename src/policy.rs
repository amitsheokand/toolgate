//! Deterministic policy: pure `decide` over normalized [`ToolEvent`]s.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{NormalizedArgs, Phase, ToolEvent, ToolKind};
use crate::gate::{self, Rules, Verdict};
use crate::read;
use crate::run;

/// Enforce applies decisions; observe logs only (vanilla arm).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyMode {
    #[default]
    Enforce,
    Observe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRule {
    #[serde(default = "default_max_lines")]
    pub max_lines: u64,
}

impl Default for ReadRule {
    fn default() -> Self {
        Self {
            max_lines: default_max_lines(),
        }
    }
}

fn default_max_lines() -> u64 {
    read::WHOLE_FILE_LIMIT_LINES
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ScopeRule {
    #[serde(default)]
    pub require_path_scope: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipRule {
    #[serde(default = "default_min_bytes")]
    pub min_bytes: usize,
}

impl Default for ClipRule {
    fn default() -> Self {
        Self {
            min_bytes: default_min_bytes(),
        }
    }
}

fn default_min_bytes() -> usize {
    4096
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellRule {
    #[serde(skip)]
    pub rules: Rules,
}

impl Default for ShellRule {
    fn default() -> Self {
        Self {
            rules: Rules::default(),
        }
    }
}

/// Full policy (TOML on disk + compiled defaults).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default)]
    pub mode: PolicyMode,
    #[serde(default)]
    pub read: ReadRule,
    #[serde(default)]
    pub glob: ScopeRule,
    #[serde(default)]
    pub grep: ScopeRule,
    #[serde(default)]
    pub clip: ClipRule,
    #[serde(default)]
    pub shell: ShellRule,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            mode: PolicyMode::Enforce,
            read: ReadRule {
                max_lines: read::WHOLE_FILE_LIMIT_LINES,
            },
            glob: ScopeRule {
                require_path_scope: true,
            },
            grep: ScopeRule {
                require_path_scope: false,
            },
            clip: ClipRule {
                min_bytes: default_min_bytes(),
            },
            shell: ShellRule::default(),
        }
    }
}

/// Policy decision before harness-specific rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny {
        agent_message: String,
        rule_id: String,
    },
    Rewrite {
        updated_input: Value,
        rule_id: String,
    },
    ReplaceOutput {
        text: String,
        rule_id: String,
    },
}

/// Raw decision plus what to apply after mode masking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyOutcome {
    pub raw: Decision,
    pub applied: Decision,
    pub rule_id: Option<String>,
}

/// Load policy from `path` when present; missing file → defaults. Parse errors → defaults.
#[must_use]
pub fn load_policy_file(path: &Path) -> Policy {
    let text = std::fs::read_to_string(path).ok();
    let Some(text) = text else {
        return Policy::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

/// Resolve mode: `TOOLGATE_POLICY` overrides file when set to `enforce` or `observe`.
#[must_use]
pub fn resolve_mode(file: &Policy, env_override: Option<&str>) -> PolicyMode {
    if let Some(v) = env_override {
        match v.trim().to_ascii_lowercase().as_str() {
            "enforce" => return PolicyMode::Enforce,
            "observe" => return PolicyMode::Observe,
            _ => {}
        }
    }
    file.mode
}

/// Pure policy evaluation (no I/O except optional archive write via `archive_dir`).
#[must_use]
pub fn decide(event: &ToolEvent, policy: &Policy, archive_dir: Option<&Path>) -> PolicyOutcome {
    let raw = decide_inner(event, policy, archive_dir);
    let rule_id = rule_id_of(&raw);
    let applied = if policy.mode == PolicyMode::Observe {
        Decision::Allow
    } else {
        raw.clone()
    };
    PolicyOutcome {
        raw,
        applied,
        rule_id,
    }
}

fn rule_id_of(d: &Decision) -> Option<String> {
    match d {
        Decision::Allow => None,
        Decision::Deny { rule_id, .. }
        | Decision::Rewrite { rule_id, .. }
        | Decision::ReplaceOutput { rule_id, .. } => Some(rule_id.clone()),
    }
}

fn decide_inner(event: &ToolEvent, policy: &Policy, archive_dir: Option<&Path>) -> Decision {
    match event.phase {
        Phase::Pre => decide_pre(event, policy),
        Phase::Post => decide_post(event, policy, archive_dir),
    }
}

fn decide_pre(event: &ToolEvent, policy: &Policy) -> Decision {
    match &event.tool {
        ToolKind::Read => read_pre(event, policy),
        ToolKind::Glob if policy.glob.require_path_scope => scope_pre(event, "glob.unscoped"),
        ToolKind::Grep if policy.grep.require_path_scope => scope_pre(event, "grep.unscoped"),
        ToolKind::Shell => shell_pre(event, &policy.shell.rules, &event.cwd),
        _ => Decision::Allow,
    }
}

fn decide_post(event: &ToolEvent, policy: &Policy, archive_dir: Option<&Path>) -> Decision {
    let output = event.output.as_deref().unwrap_or("");
    if output.len() < policy.clip.min_bytes {
        return Decision::Allow;
    }
    match &event.tool {
        ToolKind::Shell | ToolKind::Mcp { .. } => {
            clip_output_decision(output, policy.clip.min_bytes, archive_dir)
        }
        _ => Decision::Allow,
    }
}

fn read_pre(event: &ToolEvent, policy: &Policy) -> Decision {
    if event.args.offset.is_some() || event.args.limit.is_some() {
        return Decision::Allow;
    }
    let path = event.args.path.as_deref();
    let Some(path) = path else {
        return Decision::Allow;
    };
    let cwd = Path::new(&event.cwd);
    let abs = resolve_path(cwd, path);
    let lines = read::count_file_lines(&abs);
    if lines.is_none_or(|n| n <= policy.read.max_lines) {
        return Decision::Allow;
    }
    let total = lines.unwrap_or(policy.read.max_lines + 1);
    Decision::Deny {
        agent_message: format!(
            "{path} has {total} lines; whole-file reads over {} lines are capped. \
             Re-read with offset/limit for the range you need, or call one-grep `context` \
             for the enclosing symbol.",
            policy.read.max_lines
        ),
        rule_id: "read.whole_file_cap".into(),
    }
}

fn scope_pre(event: &ToolEvent, rule_id: &str) -> Decision {
    if args_has_path_scope(&event.args) {
        return Decision::Allow;
    }
    Decision::Deny {
        agent_message: format!(
            "Unscoped {} at repo root floods context. Pass a path scope (directory or \
             path-qualified pattern) so the search stays bounded.",
            rule_id.split('.').next().unwrap_or("search")
        ),
        rule_id: rule_id.to_owned(),
    }
}

fn args_has_path_scope(args: &NormalizedArgs) -> bool {
    if let Some(p) = &args.path {
        let p = p.trim();
        if !p.is_empty() && p != "." && p != "./" {
            return true;
        }
    }
    if let Some(pat) = &args.pattern {
        if pat.contains('/') || pat.contains('\\') {
            return true;
        }
    }
    false
}

fn shell_pre(event: &ToolEvent, rules: &Rules, root: &str) -> Decision {
    let cmd = event.args.command.as_deref().unwrap_or("");
    let argv = parse_shell_argv(cmd);
    let Some(argv) = argv else {
        return Decision::Allow;
    };
    if let Some(v) = gate::rules_verdict(&argv, rules, root) {
        let msg = match v {
            Verdict::Allow => return Decision::Allow,
            Verdict::Ask(m) | Verdict::Block(m) => m,
        };
        return Decision::Deny {
            agent_message: msg,
            rule_id: "shell.rules_deny".into(),
        };
    }
    Decision::Allow
}

fn parse_shell_argv(cmd: &str) -> Option<Vec<String>> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    for ch in trimmed.chars() {
        if escape {
            cur.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' && in_double {
            escape = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if ch.is_whitespace() && !in_single && !in_double {
            if !cur.is_empty() {
                out.push(cur.clone());
                cur.clear();
            }
            continue;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() { None } else { Some(out) }
}

fn clip_output_decision(output: &str, min_bytes: usize, archive_dir: Option<&Path>) -> Decision {
    if output.len() < min_bytes {
        return Decision::Allow;
    }
    let clipped = run::clip_utf8(output, run::OUTPUT_CAP_BYTES);
    let text = if let Some(dir) = archive_dir {
        if let Some(path) = archive_output(output, dir) {
            format!(
                "{clipped}\n\n[toolgate] full output archived at {}",
                path.display()
            )
        } else {
            clipped
        }
    } else {
        clipped
    };
    Decision::ReplaceOutput {
        text,
        rule_id: "clip.output".into(),
    }
}

fn archive_output(body: &str, dir: &Path) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
    if std::fs::create_dir_all(dir).is_err() {
        return None;
    }
    let path = dir.join(format!("{hash}.log"));
    if std::fs::write(&path, body).is_err() {
        return None;
    }
    Some(path)
}

fn resolve_path(cwd: &Path, raw: &str) -> PathBuf {
    if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        cwd.join(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Harness, NormalizedArgs, Phase};

    fn ev(tool: ToolKind, phase: Phase, args: NormalizedArgs) -> ToolEvent {
        ToolEvent {
            harness: Harness::Cursor,
            phase,
            tool,
            args,
            output: None,
            session_id: "s".into(),
            cwd: "/tmp/ws".into(),
        }
    }

    #[test]
    fn observe_mode_masks_decision() {
        let dir = tempfile::tempdir().expect("tempdir");
        let big = "x".repeat(5000);
        let mut policy = Policy::default();
        policy.mode = PolicyMode::Observe;
        let event = ToolEvent {
            output: Some(big),
            harness: Harness::Cursor,
            phase: Phase::Post,
            tool: ToolKind::Shell,
            args: NormalizedArgs::default(),
            session_id: "s".into(),
            cwd: dir.path().to_string_lossy().into_owned(),
        };
        let out = decide(&event, &policy, Some(dir.path()));
        assert!(matches!(out.raw, Decision::ReplaceOutput { .. }));
        assert_eq!(out.applied, Decision::Allow);
    }

    #[test]
    fn glob_unscoped_denied() {
        let policy = Policy::default();
        let event = ev(
            ToolKind::Glob,
            Phase::Pre,
            NormalizedArgs {
                pattern: Some("*.rs".into()),
                ..Default::default()
            },
        );
        let out = decide(&event, &policy, None);
        assert!(matches!(out.raw, Decision::Deny { .. }));
    }

    #[test]
    fn shell_deny_rm_rf() {
        let policy = Policy::default();
        let event = ev(
            ToolKind::Shell,
            Phase::Pre,
            NormalizedArgs {
                command: Some("rm -rf /".into()),
                ..Default::default()
            },
        );
        let out = decide(&event, &policy, None);
        assert!(matches!(out.raw, Decision::Deny { .. }));
    }

    #[test]
    fn telemetry_digest_paths_no_raw_in_policy() {
        let policy = Policy::default();
        let event = ev(
            ToolKind::Read,
            Phase::Pre,
            NormalizedArgs {
                path: Some("secret.txt".into()),
                ..Default::default()
            },
        );
        let out = decide(&event, &policy, None);
        assert_eq!(out.applied, Decision::Allow);
    }
}
