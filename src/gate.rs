//! Safety gate for [`crate::run`]: deterministic rules, then Jev Noul.
//!
//! Thresholds live in one [`Policy`] (plain data, uncalibrated — tune from
//! logs), [`decide`] is a pure function testable with canned numbers, and
//! failures fail **closed**. The ask band refuses with a "needs a person"
//! message (no human channel in this tool).
//!
//! **Residual:** allowed `git diff` / `git log` skip Jev but cannot
//! neutralize repo `diff.external`, aliases, or other config — treat logs as
//! ground truth.

#[cfg(feature = "jev")]
use std::collections::HashMap;

#[cfg(feature = "jev")]
use serde::Deserialize;
use thiserror::Error;

/// Process-env key, shared with one-grep's Jev wiring.
pub const ENV_API_KEY: &str = "TYPESAFE_API_KEY";
/// Process-env base URL override (LocalJev, Laya, ...).
pub const ENV_BASE_URL: &str = "TYPESAFE_BASE_URL";
/// Process-env model override.
pub const ENV_MODEL: &str = "JEV_MODEL";
/// Server gate mode (`off` or `jev`).
pub const ENV_GATE_MODE: &str = "TOOLGATE_GATE";

#[cfg(feature = "jev")]
const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
#[cfg(feature = "jev")]
const DEFAULT_MODEL: &str = "jev-1.13.0";
/// HTTP budget per gate call.
#[cfg(feature = "jev")]
const GATE_TIMEOUT_SECS: u64 = 10;

/// Score at or above which a command is blocked.
pub const BLOCK_AT: f64 = 0.65;
/// Score below which a command runs. The `[ask_at, block_at)` band
/// refuses with a needs-a-person message (no human channel here).
pub const ASK_AT: f64 = 0.35;

/// Whether the MCP server runs commands through the Jev gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GateMode {
    /// Run without Jev (deterministic rules still apply when mode is Jev).
    #[default]
    Off,
    /// Deterministic rules first, then Jev for the rest.
    Jev,
}

impl GateMode {
    /// Parse `off` or `jev` (case-insensitive).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "" => Some(Self::Off),
            "jev" => Some(Self::Jev),
            _ => None,
        }
    }

    /// Resolve from `TOOLGATE_GATE` when set, otherwise `default_off`.
    #[must_use]
    pub fn from_env_or(default_off: Self) -> Self {
        std::env::var(ENV_GATE_MODE)
            .ok()
            .and_then(|v| Self::parse(&v))
            .unwrap_or(default_off)
    }
}

/// argv prefix lists (documentation / custom rules); [`rules_verdict`] uses
/// structured matching on program basename and flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    /// Prefixes that always run (no Jev call).
    pub allow: Vec<Vec<String>>,
    /// Prefixes that always refuse (no Jev call).
    pub deny: Vec<Vec<String>>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            allow: vec![
                vec!["cargo".into(), "test".into()],
                vec!["cargo".into(), "check".into()],
                vec!["cargo".into(), "build".into()],
                vec!["git".into(), "diff".into()],
                vec!["git".into(), "status".into()],
                vec!["git".into(), "log".into()],
                vec!["rg".into()],
                vec!["ls".into()],
            ],
            deny: vec![
                vec!["rm".into(), "-rf".into()],
                vec!["git".into(), "push".into(), "--force".into()],
                vec!["git".into(), "reset".into(), "--hard".into()],
                vec!["dd".into()],
                vec!["mkfs".into()],
            ],
        }
    }
}

fn program_basename(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

fn arg_is(value: &str, name: &str) -> bool {
    value == name || value.starts_with(&format!("{name}="))
}

fn long_flag_name(token: &str) -> Option<&str> {
    token
        .strip_prefix("--")
        .map(|s| s.split_once('=').map_or(s, |(k, _)| k))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FlagArity {
    Flag,
    TakesValue,
    EqOrValue,
}

struct FlagDef {
    long: &'static str,
    short: Option<char>,
    arity: FlagArity,
    path_value: bool,
}

const GIT_GLOBAL_FLAGS: &[FlagDef] = &[
    FlagDef {
        long: "git-dir",
        short: None,
        arity: FlagArity::EqOrValue,
        path_value: true,
    },
    FlagDef {
        long: "work-tree",
        short: None,
        arity: FlagArity::EqOrValue,
        path_value: true,
    },
    FlagDef {
        long: "namespace",
        short: None,
        arity: FlagArity::EqOrValue,
        path_value: false,
    },
    FlagDef {
        long: "paginate",
        short: Some('p'),
        arity: FlagArity::Flag,
        path_value: false,
    },
    FlagDef {
        long: "no-pager",
        short: Some('P'),
        arity: FlagArity::Flag,
        path_value: false,
    },
    FlagDef {
        long: "bare",
        short: None,
        arity: FlagArity::Flag,
        path_value: false,
    },
    FlagDef {
        long: "no-replace-objects",
        short: None,
        arity: FlagArity::Flag,
        path_value: false,
    },
    FlagDef {
        long: "literal-pathspecs",
        short: None,
        arity: FlagArity::Flag,
        path_value: false,
    },
];

fn git_short_cluster_known(rest: &str) -> bool {
    !rest.is_empty() && rest.chars().all(|c| matches!(c, 'C' | 'P' | 'p'))
}

fn git_global_path_ok(root: &str, value: &str) -> bool {
    crate::read::inside_root(root, value).is_some()
}

/// One argv per step (two when `-C` in a short cluster needs a value).
/// When `root` is `None` (deny normalization), path-valued globals are not checked.
fn advance_git_global(args: &[String], i: &mut usize, root: Option<&str>) -> bool {
    if *i >= args.len() {
        return false;
    }
    let a = &args[*i];
    if a == "-" {
        *i += 1;
        return true;
    }
    if a == "-c" || (a.starts_with("-c") && !a.starts_with("-C")) {
        *i += 1;
        if a == "-c" && *i < args.len() {
            *i += 1;
        }
        return true;
    }
    if a == "-C" {
        *i += 1;
        if *i < args.len() {
            let v = args[*i].as_str();
            *i += 1;
            if root.is_some_and(|r| !git_global_path_ok(r, v)) {
                return true;
            }
        }
        return false;
    }
    if a.starts_with("-C") && a.len() > 2 {
        let v = &a[2..];
        *i += 1;
        return root.is_some_and(|r| !git_global_path_ok(r, v));
    }
    if let Some(name) = long_flag_name(a) {
        if GIT_GLOBAL_FLAGS.iter().any(|f| f.long == name) {
            let def = GIT_GLOBAL_FLAGS.iter().find(|f| f.long == name);
            let takes = def.map(|f| f.arity != FlagArity::Flag).unwrap_or(false);
            let path_value = def.map(|f| f.path_value).unwrap_or(false);
            let inline = a.split_once('=').map(|(_, v)| v);
            *i += 1;
            let value = if let Some(v) = inline {
                Some(v)
            } else if takes && *i < args.len() {
                let v = args[*i].as_str();
                *i += 1;
                Some(v)
            } else {
                None
            };
            if path_value {
                if let Some(v) = value {
                    if root.is_some_and(|r| !git_global_path_ok(r, v)) {
                        return true;
                    }
                }
            }
            return false;
        }
        *i += 1;
        return true;
    }
    if a.starts_with('-') && !a.starts_with("--") {
        let rest = &a[1..];
        if rest.is_empty() {
            *i += 1;
            return true;
        }
        if git_short_cluster_known(rest) {
            let needs_val = rest.contains('C');
            *i += 1;
            if needs_val {
                if *i >= args.len() {
                    return true;
                }
                let v = args[*i].as_str();
                *i += 1;
                if root.is_some_and(|r| !git_global_path_ok(r, v)) {
                    return true;
                }
            }
            return false;
        }
        *i += 1;
        return true;
    }
    false
}

fn normalized_argv(argv: &[String]) -> Vec<String> {
    if argv.is_empty() {
        return Vec::new();
    }
    let prog = program_basename(&argv[0]);
    if prog != "git" {
        return argv.to_vec();
    }
    let args = &argv[1..];
    let mut i = 0;
    while i < args.len() {
        if !args[i].starts_with('-') {
            break;
        }
        let before = i;
        if advance_git_global(args, &mut i, None) {
            break;
        }
        if i == before {
            break;
        }
    }
    let mut out = vec![argv[0].clone()];
    out.extend(args[i..].iter().cloned());
    out
}

fn expand_short_flags(args: &[String]) -> Vec<char> {
    let mut flags = Vec::new();
    for a in args {
        if a.starts_with("--") || a == "-" {
            continue;
        }
        if let Some(rest) = a.strip_prefix('-') {
            for c in rest.chars() {
                if c.is_ascii_alphabetic() {
                    flags.push(c);
                }
            }
        }
    }
    flags
}

fn has_rm_recursive_force(args: &[String]) -> bool {
    let flags = expand_short_flags(args);
    let short_r = flags.iter().any(|&c| c == 'r' || c == 'R');
    let short_f = flags.iter().any(|&c| c == 'f');
    let long_r = args.iter().any(|a| arg_is(a, "--recursive"));
    let long_f = args.iter().any(|a| arg_is(a, "--force"));
    (short_r || long_r) && (short_f || long_f)
}

fn subcommand_at(args: &[String], i: usize) -> Option<&str> {
    args.get(i).map(String::as_str)
}

fn git_push_force_on_tail(tail: &[String]) -> bool {
    if subcommand_at(tail, 0) != Some("push") {
        return false;
    }
    for a in tail.iter().skip(1) {
        if arg_is(a, "--force") || a.starts_with("--force-with-lease") {
            return true;
        }
        if a.starts_with('+') {
            return true;
        }
        if a.starts_with('-') && !a.starts_with("--") && a[1..].chars().any(|c| c == 'f') {
            return true;
        }
    }
    false
}

fn git_push_force_in_args(args: &[String]) -> bool {
    for idx in 0..args.len() {
        if args[idx] == "push" && git_push_force_on_tail(&args[idx..]) {
            return true;
        }
    }
    false
}

fn git_clean_force(tail: &[String]) -> bool {
    if subcommand_at(tail, 0) != Some("clean") {
        return false;
    }
    tail.iter().skip(1).any(|a| {
        arg_is(a, "--force")
            || arg_is(a, "-f")
            || (a.starts_with('-') && !a.starts_with("--") && a[1..].contains('f'))
    })
}

fn argv_prefix_match(argv: &[String], prefix: &[String]) -> bool {
    if argv.len() < prefix.len() {
        return false;
    }
    for (i, p) in prefix.iter().enumerate() {
        if i == 0 {
            if program_basename(&argv[0]) != program_basename(p) {
                return false;
            }
        } else if argv[i] != *p {
            return false;
        }
    }
    true
}

fn builtin_deny_on_normalized(norm: &[String]) -> Option<Verdict> {
    if norm.is_empty() {
        return None;
    }
    let prog = program_basename(&norm[0]);
    let args = &norm[1..];
    if prog == "rm" && has_rm_recursive_force(args) {
        return Some(Verdict::Block("denylist: rm recursive+force".into()));
    }
    if prog == "git" {
        if git_push_force_in_args(args) {
            return Some(Verdict::Block("denylist: git push --force".into()));
        }
        for idx in 0..args.len() {
            if args[idx] == "reset" && args[idx..].iter().any(|a| arg_is(a, "--hard")) {
                return Some(Verdict::Block("denylist: git reset --hard".into()));
            }
        }
        for idx in 0..args.len() {
            if git_clean_force(&args[idx..]) {
                return Some(Verdict::Block("denylist: git clean --force".into()));
            }
        }
    }
    if prog == "dd" || prog == "mkfs" {
        return Some(Verdict::Block(format!("denylist: {prog}")));
    }
    None
}

fn hard_deny(argv: &[String], rules: &Rules) -> Option<Verdict> {
    let norm = normalized_argv(argv);
    if let Some(v) = builtin_deny_on_normalized(&norm) {
        return Some(v);
    }
    for prefix in &rules.deny {
        if argv_prefix_match(&norm, prefix) {
            return Some(Verdict::Block(format!("denylist: {}", prefix.join(" "))));
        }
    }
    None
}

fn path_value_inside_root(root: &str, value: &str) -> bool {
    crate::read::inside_root(root, value).is_some()
}

fn advance_one_flag(args: &[String], i: &mut usize, defs: &[FlagDef], root: &str) -> Option<bool> {
    if *i >= args.len() {
        return Some(false);
    }
    let a = &args[*i];
    if let Some(name) = long_flag_name(a) {
        if matches!(name, "output" | "textconv" | "ext-diff" | "config") {
            return Some(false);
        }
        let def = defs.iter().find(|d| d.long == name)?;
        let inline = a.split_once('=').map(|(_, v)| v);
        *i += 1;
        let value = match def.arity {
            FlagArity::Flag => None,
            FlagArity::TakesValue | FlagArity::EqOrValue => {
                if let Some(v) = inline {
                    Some(v)
                } else if *i < args.len() {
                    let v = args[*i].as_str();
                    *i += 1;
                    Some(v)
                } else {
                    None
                }
            }
        };
        if def.path_value {
            if let Some(v) = value {
                if !path_value_inside_root(root, v) {
                    return Some(false);
                }
            }
        }
        return Some(true);
    }
    if a.starts_with('-') && !a.starts_with("--") {
        let rest = &a[1..];
        if rest.is_empty() {
            return Some(false);
        }
        if rest.len() == 1 {
            let ch = rest.chars().next().expect("one char");
            let def = defs.iter().find(|d| d.short == Some(ch))?;
            *i += 1;
            if def.arity != FlagArity::Flag {
                if *i >= args.len() {
                    return Some(false);
                }
                let v = args[*i].as_str();
                *i += 1;
                if def.path_value && !path_value_inside_root(root, v) {
                    return Some(false);
                }
            }
            return Some(true);
        }
        for ch in rest.chars() {
            let def = defs.iter().find(|d| d.short == Some(ch));
            match def {
                Some(d) if d.arity == FlagArity::Flag => {}
                _ => return Some(false),
            }
        }
        *i += 1;
        return Some(true);
    }
    if a.starts_with('-') {
        return Some(false);
    }
    *i += 1;
    Some(true)
}

const CARGO_SUB_FLAGS: &[FlagDef] = &[
    FlagDef {
        long: "package",
        short: Some('p'),
        arity: FlagArity::TakesValue,
        path_value: false,
    },
    FlagDef {
        long: "manifest-path",
        short: None,
        arity: FlagArity::EqOrValue,
        path_value: true,
    },
    FlagDef {
        long: "target-dir",
        short: None,
        arity: FlagArity::EqOrValue,
        path_value: true,
    },
    FlagDef {
        long: "all",
        short: None,
        arity: FlagArity::Flag,
        path_value: false,
    },
    FlagDef {
        long: "quiet",
        short: Some('q'),
        arity: FlagArity::Flag,
        path_value: false,
    },
    FlagDef {
        long: "verbose",
        short: Some('v'),
        arity: FlagArity::Flag,
        path_value: false,
    },
];

const GIT_STATUS_FLAGS: &[FlagDef] = &[
    FlagDef {
        long: "short",
        short: Some('s'),
        arity: FlagArity::Flag,
        path_value: false,
    },
    FlagDef {
        long: "branch",
        short: Some('b'),
        arity: FlagArity::Flag,
        path_value: false,
    },
];

const GIT_DIFF_FLAGS: &[FlagDef] = &[FlagDef {
    long: "stat",
    short: None,
    arity: FlagArity::Flag,
    path_value: false,
}];

const GIT_LOG_FLAGS: &[FlagDef] = &[
    FlagDef {
        long: "oneline",
        short: None,
        arity: FlagArity::Flag,
        path_value: false,
    },
    FlagDef {
        long: "max-count",
        short: Some('n'),
        arity: FlagArity::TakesValue,
        path_value: false,
    },
];

const RG_FLAGS: &[FlagDef] = &[
    FlagDef {
        long: "line-number",
        short: Some('n'),
        arity: FlagArity::Flag,
        path_value: false,
    },
    FlagDef {
        long: "files-with-matches",
        short: Some('l'),
        arity: FlagArity::Flag,
        path_value: false,
    },
];

fn cargo_sub_flags(sub: &str) -> Option<&'static [FlagDef]> {
    match sub {
        "test" | "check" | "build" => Some(CARGO_SUB_FLAGS),
        _ => None,
    }
}

fn git_sub_flags(sub: &str) -> Option<&'static [FlagDef]> {
    match sub {
        "status" => Some(GIT_STATUS_FLAGS),
        "diff" => Some(GIT_DIFF_FLAGS),
        "log" => Some(GIT_LOG_FLAGS),
        _ => None,
    }
}

fn parse_git_allow(args: &[String], root: &str) -> bool {
    let mut i = 0;
    let mut steps = 0;
    while i < args.len() && steps <= args.len() + 4 {
        if !args[i].starts_with('-') {
            break;
        }
        let before = i;
        if advance_git_global(args, &mut i, Some(root)) {
            return false;
        }
        if i == before {
            return false;
        }
        steps += 1;
    }
    if i >= args.len() {
        return false;
    }
    let sub = args[i].as_str();
    let flags = match git_sub_flags(sub) {
        Some(f) => f,
        None => return false,
    };
    i += 1;
    while i < args.len() {
        steps += 1;
        if steps > args.len() * 2 + 8 {
            return false;
        }
        if advance_one_flag(args, &mut i, flags, root) != Some(true) {
            return false;
        }
    }
    true
}

fn schema_allows(argv: &[String], root: &str) -> bool {
    if argv.is_empty() {
        return false;
    }
    let prog = program_basename(&argv[0]);
    let args = &argv[1..];
    match prog {
        "cargo" => {
            if args.is_empty() {
                return false;
            }
            let sub = args[0].as_str();
            let flags = match cargo_sub_flags(sub) {
                Some(f) => f,
                None => return false,
            };
            let mut i = 1;
            let mut steps = 0;
            while i < args.len() {
                steps += 1;
                if steps > args.len() * 2 + 8 {
                    return false;
                }
                if advance_one_flag(args, &mut i, flags, root) != Some(true) {
                    return false;
                }
            }
            !args.iter().any(|a| {
                arg_is(a, "--config")
                    || a.starts_with("-Z")
                    || a.contains("runner")
                    || a.contains("program")
                    || arg_is(a, "--pre")
            })
        }
        "git" => parse_git_allow(args, root),
        "rg" => {
            let mut i = 0;
            let mut steps = 0;
            while i < args.len() {
                steps += 1;
                if steps > args.len() * 2 + 8 {
                    return false;
                }
                if advance_one_flag(args, &mut i, RG_FLAGS, root) != Some(true) {
                    return false;
                }
            }
            true
        }
        "ls" => {
            let mut i = 0;
            while i < args.len() {
                let a = &args[i];
                if a.starts_with('-') {
                    if a == "--" {
                        i += 1;
                        continue;
                    }
                    return false;
                }
                if !path_value_inside_root(root, a) {
                    return false;
                }
                i += 1;
            }
            true
        }
        _ => false,
    }
}

fn program_has_allow_schema(prog: &str) -> bool {
    matches!(prog, "cargo" | "git" | "rg" | "ls")
}

fn hard_allow(argv: &[String], rules: &Rules, root: &str) -> Option<Verdict> {
    if schema_allows(argv, root) {
        return Some(Verdict::Allow);
    }
    if argv.len() != 1 {
        return None;
    }
    let prog = program_basename(&argv[0]);
    if program_has_allow_schema(prog) {
        return None;
    }
    for prefix in &rules.allow {
        if prefix.len() == 1 && argv_prefix_match(argv, prefix) {
            return Some(Verdict::Allow);
        }
    }
    None
}

/// Deterministic verdict from [`Rules`], or `None` when Jev should judge.
#[must_use]
pub fn rules_verdict(argv: &[String], rules: &Rules, root: &str) -> Option<Verdict> {
    if let Some(v) = hard_deny(argv, rules) {
        return Some(v);
    }
    hard_allow(argv, rules, root)
}

/// Cache key for Jev verdicts (root + argv).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GateCacheKey {
    /// Workspace root string passed to Jev.
    pub root: String,
    /// Full argv (program + args).
    pub argv: Vec<String>,
}

/// Gate errors: key/config problems fail closed at the call site.
#[derive(Debug, Error)]
pub enum Error {
    /// No API key configured.
    #[error("jev unavailable (missing `{0}`): refusing run")]
    NoKey(&'static str),
    /// HTTP or protocol failure.
    #[error("jev call failed: {0}")]
    Call(String),
}

/// Threshold policy: one place, tuned from logs, tested without a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    /// Block at or above this Noul.
    pub block_at: f64,
    /// Run below this Noul.
    pub ask_at: f64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            block_at: BLOCK_AT,
            ask_at: ASK_AT,
        }
    }
}

/// Gate verdict for a Noul safety score (higher = more dangerous).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Run the command.
    Allow,
    /// Refuse: needs a person (no human channel in this tool).
    Ask(String),
    /// Refuse: too dangerous.
    Block(String),
}

/// Pure policy: Noul in, verdict out. No I/O, no model.
#[must_use]
pub fn decide(noul: f64, policy: &Policy) -> Verdict {
    if noul >= policy.block_at {
        Verdict::Block(format!(
            "safety noul {noul:.2} >= block_at {:.2}",
            policy.block_at
        ))
    } else if noul >= policy.ask_at {
        Verdict::Ask(format!(
            "safety noul {noul:.2} in ask band [{:.2}, {:.2}): needs a person",
            policy.ask_at, policy.block_at
        ))
    } else {
        Verdict::Allow
    }
}

/// Enforce gate policy for one argv (rules, cache, optional Jev).
///
/// When `mode` is [`GateMode::Off`], returns [`Verdict::Allow`] immediately.
#[cfg(feature = "jev")]
pub async fn enforce(
    mode: GateMode,
    gate: &Gate,
    rules: &Rules,
    cache: &mut HashMap<GateCacheKey, Verdict>,
    program: &str,
    args: &[String],
    root: &str,
    policy: &Policy,
) -> Result<Verdict, Error> {
    if mode == GateMode::Off {
        return Ok(Verdict::Allow);
    }
    let argv: Vec<String> = std::iter::once(program.to_owned())
        .chain(args.iter().cloned())
        .collect();
    if let Some(v) = rules_verdict(&argv, rules, root) {
        return Ok(v);
    }
    let key = GateCacheKey {
        root: root.to_owned(),
        argv,
    };
    if let Some(v) = cache.get(&key) {
        return Ok(v.clone());
    }
    let score = gate.judge(&key.argv, root).await?;
    let verdict = decide(score, policy);
    cache.insert(key, verdict.clone());
    Ok(verdict)
}

#[cfg(feature = "jev")]
#[derive(Debug, Deserialize)]
struct NoulAnswer {
    #[serde(default)]
    noul: Option<f64>,
}

#[cfg(feature = "jev")]
const SAFETY_INSTRUCTIONS: &str = "Is this command unsafe to run in the workspace?";
#[cfg(feature = "jev")]
const SAFETY_TRUE: &str = "Unsafe: deleting or overwriting data outside normal build outputs, writing outside \
     the workspace root, network egress of workspace data, privilege changes, or other \
     hard-to-undo effects.";
#[cfg(feature = "jev")]
const SAFETY_FALSE: &str = "Safe: reading, building, and running tests inside the workspace root; inspecting \
     state without hard-to-undo side effects outside build outputs.";

/// Minimal async SystemOne client (Noul only).
#[cfg(feature = "jev")]
#[derive(Debug)]
pub struct Gate {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

#[cfg(feature = "jev")]
impl Gate {
    /// Build a client with explicit credentials (tests use this).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Call`] when the HTTP client cannot be built.
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, Error> {
        let api_key = api_key.into();
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        let model = model.into();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(GATE_TIMEOUT_SECS))
            .build()
            .map_err(|e| Error::Call(e.to_string()))?;
        Ok(Self {
            client,
            api_key,
            base_url,
            model,
        })
    }

    /// Build from the environment. Missing key is an error (fail closed).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoKey`] when `TYPESAFE_API_KEY` is unset.
    pub fn from_env() -> Result<Self, Error> {
        let api_key = std::env::var(ENV_API_KEY)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or(Error::NoKey(ENV_API_KEY))?;
        let base_url = std::env::var(ENV_BASE_URL)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let model = std::env::var(ENV_MODEL)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        Self::new(api_key, base_url, model)
    }

    /// Judge a command: P(this command is unsafe to run).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Call`] on HTTP or protocol failures.
    pub async fn judge(&self, argv: &[String], root: &str) -> Result<f64, Error> {
        let body = serde_json::json!({
            "model": self.model,
            "state": {"argv": argv, "root": root},
            "questions": {
                "safety": {
                    "type": "noul",
                    "instructions": SAFETY_INSTRUCTIONS,
                    "criteria": {
                        "true": SAFETY_TRUE,
                        "false": SAFETY_FALSE,
                    },
                },
            },
        });
        let response = self
            .client
            .post(format!("{}/v1/systemone", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Call(e.to_string()))?;
        if !response.status().is_success() {
            return Err(Error::Call(format!("systemone HTTP {}", response.status())));
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::Call(e.to_string()))?;
        value
            .get("answers")
            .and_then(|a| a.get("safety"))
            .and_then(|s| serde_json::from_value::<NoulAnswer>(s.clone()).ok())
            .and_then(|a| a.noul)
            .ok_or_else(|| Error::Call("missing safety noul in response".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use super::*;

    fn test_root() -> &'static str {
        static ROOT: OnceLock<PathBuf> = OnceLock::new();
        ROOT.get_or_init(|| {
            let p =
                std::env::temp_dir().join(format!("toolgate-allow-root-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("test root");
            p
        })
        .to_str()
        .expect("utf8 root")
    }

    #[test]
    fn policy_matches_article_numbers() {
        let policy = Policy::default();
        assert!(matches!(decide(0.83, &policy), Verdict::Block(_)));
        assert!(matches!(decide(0.01, &policy), Verdict::Allow));
        assert!(matches!(decide(0.70, &policy), Verdict::Block(_)));
        assert!(matches!(decide(0.34, &policy), Verdict::Allow));
        assert!(matches!(decide(0.35, &policy), Verdict::Ask(_)));
        assert!(matches!(decide(0.65, &policy), Verdict::Block(_)));
    }

    #[test]
    fn rules_allow_cargo_test() {
        let rules = Rules::default();
        let argv = vec!["cargo".into(), "test".into(), "-p".into(), "foo".into()];
        assert!(matches!(
            rules_verdict(&argv, &rules, test_root()),
            Some(Verdict::Allow)
        ));
    }

    #[test]
    fn rules_deny_rm_rf_variants() {
        let rules = Rules::default();
        for argv in [
            vec!["rm".into(), "-rf".into(), "/".into()],
            vec!["rm".into(), "-fr".into(), "/".into()],
            vec!["rm".into(), "-r".into(), "-f".into()],
            vec!["/bin/rm".into(), "--recursive".into(), "--force".into()],
        ] {
            assert!(
                matches!(
                    rules_verdict(&argv, &rules, test_root()),
                    Some(Verdict::Block(_))
                ),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn allowlist_bypasses_need_jev() {
        let rules = Rules::default();
        let cases: Vec<Vec<String>> = vec![
            vec!["rg".into(), "--pre".into(), "sh".into(), "x".into()],
            vec!["rg".into(), "--pre-glob".into(), "*.sh".into()],
            vec![
                "cargo".into(),
                "test".into(),
                "--config".into(),
                "x.toml".into(),
            ],
            vec![
                "cargo".into(),
                "test".into(),
                "-p".into(),
                "my-runner-crate".into(),
            ],
            vec!["git".into(), "diff".into(), "--output".into(), "x".into()],
            vec!["git".into(), "log".into(), "--ext-diff".into()],
            vec!["git".into(), "diff".into(), "--textconv".into()],
            vec!["git".into(), "fetch".into()],
            vec!["env".into(), "FOO=bar".into()],
            vec!["bash".into(), "-c".into(), "echo".into()],
            vec![
                "find".into(),
                ".".into(),
                "-exec".into(),
                "rm".into(),
                "{}".into(),
                ";".into(),
            ],
        ];
        for argv in cases {
            assert!(
                rules_verdict(&argv, &rules, test_root()).is_none(),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn git_status_without_config_still_allowed() {
        let rules = Rules::default();
        let argv = vec!["git".into(), "status".into()];
        assert!(matches!(
            rules_verdict(&argv, &rules, test_root()),
            Some(Verdict::Allow)
        ));
    }

    #[test]
    fn rules_deny_git_push_force_after_global_options() {
        let rules = Rules::default();
        for argv in [
            vec![
                "git".into(),
                "-C".into(),
                "d".into(),
                "push".into(),
                "--force".into(),
            ],
            vec![
                "git".into(),
                "--no-pager".into(),
                "push".into(),
                "-f".into(),
            ],
        ] {
            assert!(
                matches!(
                    rules_verdict(&argv, &rules, test_root()),
                    Some(Verdict::Block(_))
                ),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn rules_custom_allow_deny_lists() {
        let rules = Rules {
            allow: vec![vec!["echo".into()]],
            deny: vec![vec!["wget".into()]],
        };
        assert!(matches!(
            rules_verdict(&vec!["echo".into()], &rules, test_root()),
            Some(Verdict::Allow)
        ));
        assert!(rules_verdict(&vec!["echo".into(), "hi".into()], &rules, test_root()).is_none());
        assert!(matches!(
            rules_verdict(
                &vec!["wget".into(), "https://x".into()],
                &rules,
                test_root()
            ),
            Some(Verdict::Block(_))
        ));
        assert!(rules_verdict(&vec!["curl".into(), "x".into()], &rules, test_root()).is_none());
    }

    #[test]
    fn rules_unknown_needs_jev() {
        let rules = Rules::default();
        let argv = vec!["curl".into(), "https://example.com".into()];
        assert!(rules_verdict(&argv, &rules, test_root()).is_none());
    }

    #[test]
    fn git_push_force_denies_table() {
        let rules = Rules::default();
        let cases: Vec<Vec<String>> = vec![
            vec!["git".into(), "push".into(), "--force".into()],
            vec!["git".into(), "push".into(), "-f".into()],
            vec!["git".into(), "push".into(), "-fu".into(), "origin".into()],
            vec!["git".into(), "push".into(), "--force-with-lease".into()],
            vec![
                "git".into(),
                "push".into(),
                "--force-with-lease=main".into(),
            ],
            vec!["git".into(), "push".into(), "+main:main".into()],
            vec![
                "git".into(),
                "-C".into(),
                "d".into(),
                "push".into(),
                "--force".into(),
            ],
            vec![
                "git".into(),
                "--no-pager".into(),
                "push".into(),
                "-f".into(),
            ],
            vec![
                "git".into(),
                "--weird-global".into(),
                "push".into(),
                "-f".into(),
            ],
        ];
        for argv in cases {
            assert!(
                matches!(
                    rules_verdict(&argv, &rules, test_root()),
                    Some(Verdict::Block(_))
                ),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn git_unknown_global_never_allow() {
        let rules = Rules::default();
        let argv = vec!["git".into(), "--weird-global".into(), "status".into()];
        assert!(rules_verdict(&argv, &rules, test_root()).is_none());
    }

    #[test]
    fn rules_allow_git_prefix_still_denies_push_force() {
        let rules = Rules {
            allow: vec![vec!["git".into()]],
            deny: vec![],
        };
        let argv = vec!["git".into(), "push".into(), "-f".into()];
        assert!(matches!(
            rules_verdict(&argv, &rules, test_root()),
            Some(Verdict::Block(_))
        ));
    }

    #[test]
    fn git_pager_cluster_does_not_skip_push_force_deny() {
        let rules = Rules::default();
        let argv = vec!["git".into(), "-pp".into(), "push".into(), "--force".into()];
        assert!(matches!(
            rules_verdict(&argv, &rules, test_root()),
            Some(Verdict::Block(_))
        ));
    }

    #[test]
    fn git_bare_dash_argv_does_not_loop() {
        let rules = Rules::default();
        for argv in [
            vec!["git".into(), "-".into(), "diff".into()],
            vec!["git".into(), "-".into(), "push".into(), "-f".into()],
        ] {
            let v = rules_verdict(&argv, &rules, test_root());
            if argv.contains(&"-f".into()) || argv.iter().any(|a| a == "push") {
                assert!(matches!(v, Some(Verdict::Block(_))), "{argv:?}");
            } else {
                assert!(v.is_none(), "{argv:?}");
            }
        }
    }

    #[test]
    fn rules_deny_matches_normalized_git_prefix() {
        let rules = Rules {
            allow: vec![],
            deny: vec![vec!["git".into(), "push".into()]],
        };
        let argv = vec![
            "git".into(),
            "-C".into(),
            "d".into(),
            "push".into(),
            "origin".into(),
        ];
        assert!(matches!(
            rules_verdict(&argv, &rules, test_root()),
            Some(Verdict::Block(_))
        ));
    }

    #[test]
    fn cargo_paths_outside_root_need_jev() {
        let rules = Rules::default();
        let root = test_root();
        let outside = "/outside/Cargo.toml";
        for argv in [
            vec![
                "cargo".into(),
                "test".into(),
                "--manifest-path".into(),
                outside.into(),
            ],
            vec![
                "cargo".into(),
                "build".into(),
                "--target-dir".into(),
                "/outside".into(),
            ],
            vec![
                "cargo".into(),
                "test".into(),
                "--manifest-path".into(),
                "../outside/Cargo.toml".into(),
            ],
            vec![
                "cargo".into(),
                "build".into(),
                "--target-dir".into(),
                "../out".into(),
            ],
            vec![
                "cargo".into(),
                "build".into(),
                "--target-dir".into(),
                format!("{root}/../../tmp/new"),
            ],
        ] {
            assert!(rules_verdict(&argv, &rules, root).is_none(), "{argv:?}");
        }
    }

    #[test]
    fn git_config_injection_globals_not_allowed() {
        let rules = Rules::default();
        let root = test_root();
        for argv in [
            vec![
                "git".into(),
                "-c".into(),
                "diff.external=/bin/sh".into(),
                "diff".into(),
            ],
            vec![
                "git".into(),
                "-c".into(),
                "core.fsmonitor=/bin/sh".into(),
                "status".into(),
            ],
            vec![
                "git".into(),
                "--config-env".into(),
                "X=Y".into(),
                "status".into(),
            ],
        ] {
            assert!(rules_verdict(&argv, &rules, root).is_none(), "{argv:?}");
        }
    }

    #[test]
    fn ls_with_flags_outside_root_not_allowed() {
        let rules = Rules::default();
        let argv = vec!["ls".into(), "-R".into(), "/etc".into()];
        assert!(rules_verdict(&argv, &rules, test_root()).is_none());
    }

    #[test]
    fn rules_allow_prefix_never_bypasses_schema() {
        let rules = Rules {
            allow: vec![
                vec![
                    "cargo".into(),
                    "build".into(),
                    "--config".into(),
                    "build.rustc=/bin/sh".into(),
                ],
                vec!["git".into(), "diff".into(), "--textconv".into()],
            ],
            deny: vec![],
        };
        let root = test_root();
        for argv in [
            vec![
                "cargo".into(),
                "build".into(),
                "--config".into(),
                "build.rustc=/bin/sh".into(),
            ],
            vec!["git".into(), "diff".into(), "--textconv".into()],
        ] {
            assert!(rules_verdict(&argv, &rules, root).is_none(), "{argv:?}");
        }
    }

    #[test]
    fn cargo_paths_through_symlink_dotdot_not_allowed() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let root = root_dir.path().to_str().expect("utf8 root");
        let outside = tempfile::tempdir().expect("outside");
        std::os::unix::fs::symlink(outside.path(), root_dir.path().join("link")).expect("symlink");
        let rules = Rules::default();
        for argv in [
            vec![
                "cargo".into(),
                "build".into(),
                "--target-dir".into(),
                "link/../out".into(),
            ],
            vec![
                "cargo".into(),
                "test".into(),
                "--manifest-path".into(),
                "link/../out/Cargo.toml".into(),
            ],
        ] {
            assert!(rules_verdict(&argv, &rules, root).is_none(), "{argv:?}");
        }
    }

    #[test]
    fn rg_safe_flags_allow_pre_flags_do_not() {
        let rules = Rules::default();
        assert!(matches!(
            rules_verdict(
                &vec!["rg".into(), "fn".into(), "src".into()],
                &rules,
                test_root()
            ),
            Some(Verdict::Allow)
        ));
        for argv in [
            vec!["rg".into(), "--pre".into(), "sh".into(), "x".into()],
            vec!["rg".into(), "--pre-glob".into(), "*.sh".into()],
        ] {
            assert!(
                rules_verdict(&argv, &rules, test_root()).is_none(),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn schema_table_cargo_git_ls() {
        let rules = Rules::default();
        let allow_cases: Vec<Vec<String>> = vec![
            vec!["cargo".into(), "check".into()],
            vec!["cargo".into(), "test".into(), "-p".into(), "foo".into()],
            vec!["git".into(), "diff".into(), "--stat".into()],
            vec!["ls".into(), "src".into()],
        ];
        for argv in allow_cases {
            assert!(
                matches!(
                    rules_verdict(&argv, &rules, test_root()),
                    Some(Verdict::Allow)
                ),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn schema_fuzz_token_pool_bounded() {
        let rules = Rules::default();
        let pool: Vec<String> = vec![
            "cargo".into(),
            "test".into(),
            "-p".into(),
            "foo".into(),
            "--manifest-path".into(),
            format!("{}/Cargo.toml", test_root()),
            "--target-dir".into(),
            format!("{}/target", test_root()),
            "--pre".into(),
            "--output=x".into(),
            "-".into(),
            "-pp".into(),
            "git".into(),
            "diff".into(),
            "push".into(),
            "-f".into(),
            "+ref".into(),
            "/outside/x".into(),
            "../x".into(),
            format!("{}/../../x", test_root()),
            "-c".into(),
            "k=v".into(),
        ];
        let mut rng_state: u64 = 0xDEAD_BEEF;
        for _ in 0..200 {
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = (rng_state % 8) as usize + 1;
            let mut argv = Vec::new();
            for j in 0..len {
                let idx = ((rng_state >> (j * 3)) as usize) % pool.len();
                argv.push(pool[idx].clone());
            }
            let steps_before = argv.len();
            let v = rules_verdict(&argv, &rules, test_root());
            assert!(steps_before <= 16);
            if matches!(v, Some(Verdict::Allow)) {
                assert!(schema_allows(&argv, test_root()), "{argv:?}");
            }
        }
    }

    #[test]
    fn gate_mode_parse() {
        assert_eq!(GateMode::parse("jev"), Some(GateMode::Jev));
        assert_eq!(GateMode::parse("OFF"), Some(GateMode::Off));
        assert_eq!(GateMode::parse("nope"), None);
    }

    #[cfg(feature = "jev")]
    mod jev {
        use super::*;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        #[test]
        fn no_key_error_names_env_var() {
            let err = Error::NoKey(ENV_API_KEY);
            assert!(err.to_string().contains(ENV_API_KEY));
        }

        #[tokio::test]
        async fn judge_round_trips_systemone_wire() {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            let addr = listener.local_addr().expect("addr");
            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut buf = vec![0u8; 65536];
                let n = stream.read(&mut buf).await.expect("read");
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(req.contains("\"argv\""), "argv must be JSON array");
                let body = r#"{"answers":{"safety":{"type":"noul","noul":0.83}}}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
            let gate =
                Gate::new("mock-key", format!("http://{addr}"), DEFAULT_MODEL).expect("gate");
            let argv = vec!["rm".into(), "-rf".into()];
            let score = gate.judge(&argv, "/tmp").await.expect("judge");
            assert!((score - 0.83).abs() < 1e-9);
            assert!(matches!(
                decide(score, &Policy::default()),
                Verdict::Block(_)
            ));
        }

        #[tokio::test]
        async fn enforce_caches_by_root_and_argv() {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            let addr = listener.local_addr().expect("addr");
            let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let hits_c = hits.clone();
            tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.expect("accept");
                    hits_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let mut buf = vec![0u8; 65536];
                    let _ = stream.read(&mut buf).await;
                    let body = r#"{"answers":{"safety":{"type":"noul","noul":0.1}}}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
            let gate =
                Gate::new("mock-key", format!("http://{addr}"), DEFAULT_MODEL).expect("gate");
            let rules = Rules::default();
            let mut cache = HashMap::new();
            let policy = Policy::default();
            let v1 = enforce(
                GateMode::Jev,
                &gate,
                &rules,
                &mut cache,
                "curl",
                &["http://x".into()],
                "/tmp/a",
                &policy,
            )
            .await
            .expect("enforce");
            assert!(matches!(v1, Verdict::Allow));
            let _v2 = enforce(
                GateMode::Jev,
                &gate,
                &rules,
                &mut cache,
                "curl",
                &["http://x".into()],
                "/tmp/b",
                &policy,
            )
            .await
            .expect("enforce");
            let v3 = enforce(
                GateMode::Jev,
                &gate,
                &rules,
                &mut cache,
                "curl",
                &["http://x".into()],
                "/tmp/a",
                &policy,
            )
            .await
            .expect("enforce");
            assert_eq!(v1, v3);
            assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        }
    }
}
