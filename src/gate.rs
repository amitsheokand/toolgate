//! Jev safety gate for [`crate::run`]: ask a Noul, enforce a policy.
//!
//! Shape follows the harness article: thresholds live in one [`Policy`]
//! (plain data, tuned from logs), [`decide`] is a pure function testable
//! with canned numbers, and failures fail **closed** — a gate that cannot
//! ask refuses the run. There is no human channel here (single-player
//! mode), so the ask band refuses with a "needs a person" message instead
//! of queueing.

use serde::Deserialize;
use thiserror::Error;

/// Process-env key, shared with one-grep's Jev wiring.
pub const ENV_API_KEY: &str = "TYPESAFE_API_KEY";
/// Process-env base URL override (LocalJev, Laya, ...).
pub const ENV_BASE_URL: &str = "TYPESAFE_BASE_URL";
/// Process-env model override.
pub const ENV_MODEL: &str = "JEV_MODEL";
const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const DEFAULT_MODEL: &str = "jev-1.13.0";
/// HTTP budget per gate call.
const GATE_TIMEOUT_SECS: u64 = 10;

/// Score at or above which a command is blocked.
pub const BLOCK_AT: f64 = 0.65;
/// Score below which a command runs. The `[ask_at, block_at)` band
/// refuses with a needs-a-person message (no human channel here).
pub const ASK_AT: f64 = 0.35;

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

#[derive(Debug, Deserialize)]
struct NoulAnswer {
    #[serde(default)]
    noul: Option<f64>,
}

/// Minimal async SystemOne client (Noul only).
#[derive(Debug)]
pub struct Gate {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl Gate {
    /// Build from the environment. Missing key is an error now (fail
    /// closed at the call site, not a silent pass).
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
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(GATE_TIMEOUT_SECS))
            .build()
            .map_err(|e| Error::Call(e.to_string()))?;
        Ok(Self {
            client,
            api_key,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
        })
    }

    /// Judge a command: P(this command is unsafe to run).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Call`] on HTTP or protocol failures.
    pub async fn judge(&self, program: &str, args: &[String], root: &str) -> Result<f64, Error> {
        let command = std::iter::once(program.to_owned())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        let body = serde_json::json!({
            "model": self.model,
            "state": {"command": command, "root": root},
            "questions": {
                "safety": {
                    "type": "noul",
                    "instructions": "Is this command unsafe to run?",
                    "criteria": {
                        "true": "The command writes, deletes, executes untrusted input, exfiltrates data, or is hard to undo.",
                        "false": "The command only reads or inspects; it changes nothing and sends nothing outward.",
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
        // Article's calibration: delete ~0.83, read ~0.01, new-file ~0.70.
        assert!(matches!(decide(0.83, &policy), Verdict::Block(_)));
        assert!(matches!(decide(0.01, &policy), Verdict::Allow));
        assert!(matches!(decide(0.70, &policy), Verdict::Block(_)));
        // Bands: below ask runs, edges belong to the stricter side.
        assert!(matches!(decide(0.34, &policy), Verdict::Allow));
        assert!(matches!(decide(0.35, &policy), Verdict::Ask(_)));
        assert!(matches!(decide(0.65, &policy), Verdict::Block(_)));
    }

    #[test]
    fn missing_key_fails_closed() {
        // Preserve ambient key across the probe.
        let prev = std::env::var_os(ENV_API_KEY);
        unsafe {
            std::env::remove_var(ENV_API_KEY);
        }
        let err = Gate::from_env().expect_err("no key must fail");
        assert!(matches!(err, Error::NoKey(_)));
        assert!(err.to_string().contains(ENV_API_KEY));
        unsafe {
            match prev {
                Some(v) => std::env::set_var(ENV_API_KEY, v),
                None => std::env::remove_var(ENV_API_KEY),
            }
        }
    }

    #[tokio::test]
    async fn judge_round_trips_systemone_wire() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buf = vec![0u8; 65536];
            let _ = stream.read(&mut buf).await;
            let body = r#"{"answers":{"safety":{"type":"noul","noul":0.83}}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
        let prev_key = std::env::var_os(ENV_API_KEY);
        let prev_url = std::env::var_os(ENV_BASE_URL);
        unsafe {
            std::env::set_var(ENV_API_KEY, "mock-key");
            std::env::set_var(ENV_BASE_URL, format!("http://{addr}"));
        }
        let gate = Gate::from_env().expect("gate");
        let score = gate
            .judge("rm", &["-rf".to_owned()], "/tmp")
            .await
            .expect("judge");
        unsafe {
            match prev_key {
                Some(v) => std::env::set_var(ENV_API_KEY, v),
                None => std::env::remove_var(ENV_API_KEY),
            }
            match prev_url {
                Some(v) => std::env::set_var(ENV_BASE_URL, v),
                None => std::env::remove_var(ENV_BASE_URL),
            }
        }
        assert!((score - 0.83).abs() < 1e-9);
        assert!(matches!(
            decide(score, &Policy::default()),
            Verdict::Block(_)
        ));
    }
}
