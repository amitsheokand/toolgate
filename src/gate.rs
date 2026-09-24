//! Safety gate for [`crate::run`]: deterministic rules, then Jev Noul.
//!
//! Thresholds live in one [`Policy`] (plain data, uncalibrated — tune from
//! logs), [`decide`] is a pure function testable with canned numbers, and
//! failures fail **closed**. The ask band refuses with a "needs a person"
//! message (no human channel in this tool).

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

/// argv prefix lists: allow runs without Jev; deny refuses without Jev.
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

fn argv_prefix_match(argv: &[String], prefix: &[String]) -> bool {
    if argv.len() < prefix.len() {
        return false;
    }
    argv.iter().zip(prefix.iter()).all(|(a, b)| a == b)
}

/// Deterministic verdict from [`Rules`], or `None` when Jev should judge.
#[must_use]
pub fn rules_verdict(argv: &[String], rules: &Rules) -> Option<Verdict> {
    for prefix in &rules.deny {
        if argv_prefix_match(argv, prefix) {
            return Some(Verdict::Block(format!(
                "denylist prefix: {}",
                prefix.join(" ")
            )));
        }
    }
    for prefix in &rules.allow {
        if argv_prefix_match(argv, prefix) {
            return Some(Verdict::Allow);
        }
    }
    None
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
    cache: &mut HashMap<Vec<String>, Verdict>,
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
    if let Some(v) = rules_verdict(&argv, rules) {
        return Ok(v);
    }
    if let Some(v) = cache.get(&argv) {
        return Ok(v.clone());
    }
    let score = gate.judge(&argv, root).await?;
    let verdict = decide(score, policy);
    cache.insert(argv, verdict.clone());
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
    use super::*;

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
        assert!(matches!(rules_verdict(&argv, &rules), Some(Verdict::Allow)));
    }

    #[test]
    fn rules_deny_rm_rf() {
        let rules = Rules::default();
        let argv = vec!["rm".into(), "-rf".into(), "/".into()];
        assert!(matches!(
            rules_verdict(&argv, &rules),
            Some(Verdict::Block(_))
        ));
    }

    #[test]
    fn rules_unknown_needs_jev() {
        let rules = Rules::default();
        let argv = vec!["curl".into(), "https://example.com".into()];
        assert!(rules_verdict(&argv, &rules).is_none());
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
        async fn enforce_caches_jev_verdict() {
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
                "/tmp",
                &policy,
            )
            .await
            .expect("enforce");
            assert!(matches!(v1, Verdict::Allow));
            let v2 = enforce(
                GateMode::Jev,
                &gate,
                &rules,
                &mut cache,
                "curl",
                &["http://x".into()],
                "/tmp",
                &policy,
            )
            .await
            .expect("enforce");
            assert_eq!(v1, v2);
            assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }
}
