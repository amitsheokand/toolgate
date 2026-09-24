//! Bounded command execution: timeouts plus an output budget.
//!
//! Long or chatty commands are the other context hog (the shell row on the
//! chart). `run` kills on timeout and head-truncates output, returning the
//! exit code, the bounded output, and whether it timed out — so callers
//! see a verdict instead of a megabyte. argv-direct: no shell, no
//! interpolation, nothing to inject through.

use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::Serialize;
use thiserror::Error;

/// Default wall-clock budget per command.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Default captured-output budget (stdout+stderr each).
pub const OUTPUT_CAP_BYTES: usize = 64 * 1024;

/// Run errors: spawn failures and timeouts. Nonzero exits are data
/// (see [`RunHit::code`]), not errors.
#[derive(Debug, Error)]
pub enum Error {
    /// Spawn failed or the workspace is not a directory.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Empty program.
    #[error("invalid input: empty program")]
    EmptyProgram,
    /// Wall-clock budget exhausted (process killed).
    #[error("timeout after {0}s")]
    Timeout(u64),
}

/// One bounded execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunHit {
    /// Process exit code (`-1` when killed on timeout).
    pub code: i32,
    /// Stdout, head-truncated to the budget.
    pub stdout: String,
    /// Stderr, head-truncated to the budget.
    pub stderr: String,
    /// True when the process was killed for exceeding the budget.
    pub timed_out: bool,
    /// Wall time in whole milliseconds.
    pub elapsed_ms: u64,
}

fn truncate(bytes: &[u8], cap: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= cap {
        return text.into_owned();
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... (output truncated)", &text[..end])
}

/// Run `program` with `args` in `root`, killing after `timeout_secs`.
///
/// # Errors
///
/// Returns [`Error::EmptyProgram`] for empty programs, [`Error::Io`] when
/// spawning fails, [`Error::Timeout`] (after killing) on expiry. A
/// nonzero exit is a normal [`RunHit`] with `timed_out: false`.
pub fn run(
    root: &Path,
    program: &str,
    args: &[String],
    timeout_secs: u64,
    output_cap: usize,
) -> Result<RunHit, Error> {
    if program.trim().is_empty() {
        return Err(Error::EmptyProgram);
    }
    if !root.is_dir() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("workspace is not a directory: {}", root.display()),
        )));
    }
    let started = Instant::now();
    let mut child = Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Drain pipes on threads: polling try_wait without reading deadlocks
    // once the 64 KiB pipe buffers fill.
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = out_pipe.as_mut() {
            use std::io::Read as _;
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = err_pipe.as_mut() {
            use std::io::Read as _;
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let deadline = Duration::from_secs(timeout_secs.max(1));
    loop {
        match child.try_wait()? {
            Some(status) => {
                let stdout = out_reader.join().unwrap_or_default();
                let stderr = err_reader.join().unwrap_or_default();
                return Ok(RunHit {
                    code: status.code().unwrap_or(-1),
                    stdout: truncate(&stdout, output_cap),
                    stderr: truncate(&stderr, output_cap),
                    timed_out: false,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                });
            }
            None if started.elapsed() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Timeout(timeout_secs));
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_returns_output_and_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hit = run(dir.path(), "echo", &["hi".to_owned()], 10, OUTPUT_CAP_BYTES).expect("run");
        assert_eq!(hit.code, 0);
        assert!(!hit.timed_out);
        assert!(hit.stdout.contains("hi"));
    }

    #[test]
    fn nonzero_exit_is_data_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hit = run(dir.path(), "false", &[], 10, OUTPUT_CAP_BYTES).expect("run");
        assert_eq!(hit.code, 1);
        assert!(!hit.timed_out);
    }

    #[test]
    fn timeout_kills_and_reports() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = run(dir.path(), "sleep", &["30".to_owned()], 1, OUTPUT_CAP_BYTES)
            .expect_err("must time out");
        assert!(matches!(err, Error::Timeout(1)));
    }

    #[test]
    fn big_output_is_truncated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hit = run(
            dir.path(),
            "bash",
            &["-c".to_owned(), "yes | head -c 200000".to_owned()],
            10,
            1024,
        )
        .expect("run");
        assert!(hit.stdout.len() <= 1100);
        assert!(hit.stdout.contains("truncated"));
    }

    #[test]
    fn empty_program_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = run(dir.path(), "  ", &[], 10, OUTPUT_CAP_BYTES).expect_err("empty");
        assert!(matches!(err, Error::EmptyProgram));
    }
}
