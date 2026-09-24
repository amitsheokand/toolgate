//! Bounded command execution: timeouts plus an output budget.
//!
//! Long or chatty commands are the other context hog (the shell row on the
//! chart). `run` kills on timeout and head+tail-clips output, returning the
//! exit code, the bounded output, and whether it timed out — so callers
//! see a verdict instead of a megabyte. argv-direct: no shell, no
//! interpolation, nothing to inject through.

use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::Serialize;
use thiserror::Error;

/// Default wall-clock budget per command.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Default captured-output budget per stream (stdout and stderr each).
pub const OUTPUT_CAP_BYTES: usize = 8 * 1024;
/// Default head portion of [`OUTPUT_CAP_BYTES`] (remainder is tail).
pub const OUTPUT_HEAD_BYTES: usize = 2 * 1024;
/// Default tail portion of [`OUTPUT_CAP_BYTES`].
pub const OUTPUT_TAIL_BYTES: usize = OUTPUT_CAP_BYTES - OUTPUT_HEAD_BYTES;

/// Run errors: spawn failures only. Nonzero exits and timeouts are data
/// (see [`RunHit`]), not errors.
#[derive(Debug, Error)]
pub enum Error {
    /// Spawn failed or the workspace is not a directory.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Empty program.
    #[error("invalid input: empty program")]
    EmptyProgram,
}

/// One bounded execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunHit {
    /// Process exit code (`-1` when killed on timeout).
    pub code: i32,
    /// Stdout, head+tail clipped to the budget.
    pub stdout: String,
    /// Stderr, head+tail clipped to the budget.
    pub stderr: String,
    /// True when the process was killed for exceeding the budget.
    pub timed_out: bool,
    /// Wall time in whole milliseconds.
    pub elapsed_ms: u64,
}

/// Split `output_cap` into head and tail budgets (2:6 ratio at default cap).
#[must_use]
pub fn head_tail_caps(output_cap: usize) -> (usize, usize) {
    let head = output_cap / 4;
    (head, output_cap - head)
}

struct TailRing {
    cap: usize,
    buf: Vec<u8>,
    start: usize,
    len: usize,
}

impl TailRing {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            buf: Vec::with_capacity(cap.min(4096)),
            start: 0,
            len: 0,
        }
    }

    fn push(&mut self, b: u8) {
        if self.cap == 0 {
            return;
        }
        if self.len < self.cap {
            self.buf.push(b);
            self.len += 1;
        } else {
            self.buf[self.start] = b;
            self.start = (self.start + 1) % self.cap;
        }
    }

    fn ordered(&self) -> Vec<u8> {
        if self.len == 0 {
            return Vec::new();
        }
        if self.len < self.cap {
            return self.buf.clone();
        }
        let mut out = Vec::with_capacity(self.len);
        for i in 0..self.len {
            out.push(self.buf[(self.start + i) % self.cap]);
        }
        out
    }
}

struct StreamClipper {
    head_cap: usize,
    tail_cap: usize,
    head: Vec<u8>,
    tail: TailRing,
    total: usize,
}

impl StreamClipper {
    fn new(head_cap: usize, tail_cap: usize) -> Self {
        Self {
            head_cap,
            tail_cap,
            head: Vec::with_capacity(head_cap.min(4096)),
            tail: TailRing::new(tail_cap),
            total: 0,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        for &b in chunk {
            self.total += 1;
            if self.head.len() < self.head_cap {
                self.head.push(b);
            } else {
                self.tail.push(b);
            }
        }
    }

    fn finish(&self) -> String {
        if self.total == 0 {
            return String::new();
        }
        if self.total <= self.head_cap {
            return bytes_to_str(&self.head);
        }
        let tail_bytes = self.tail.ordered();
        if self.total <= self.head_cap + self.tail_cap {
            let mut combined = self.head.clone();
            combined.extend_from_slice(&tail_bytes);
            return bytes_to_str(&combined);
        }
        let head_limit = self.head_cap.min(self.head.len());
        let head_end = snap_end(&self.head[..head_limit]);
        let head_kept = head_end;
        let take = self.tail_cap.min(tail_bytes.len());
        let tail_window = &tail_bytes[tail_bytes.len() - take..];
        let tail_start = snap_start(tail_window);
        let tail_end = snap_end(tail_window).max(tail_start);
        let tail_kept = tail_end - tail_start;
        let elided = self.total - head_kept - tail_kept;
        let head_s = bytes_to_str(&self.head[..head_end]);
        let tail_s = bytes_to_str(&tail_window[tail_start..tail_end]);
        format!("{head_s}... [{elided} bytes elided] ...{tail_s}")
    }
}

fn bytes_to_str(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .expect("snap_start/snap_end yield valid UTF-8")
}

/// Skip up to three leading UTF-8 continuation bytes (never panics).
fn snap_start(bytes: &[u8]) -> usize {
    let mut i = 0usize;
    while i < bytes.len() && i < 3 && is_utf8_continuation(bytes[i]) {
        i += 1;
    }
    i
}

/// Length of a valid UTF-8 prefix; drops at most three trailing bytes (never panics).
fn snap_end(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    if std::str::from_utf8(bytes).is_ok() {
        return bytes.len();
    }
    let len = bytes.len();
    let max_trim = 3.min(len);
    for trim in 1..=max_trim {
        let end = len - trim;
        if std::str::from_utf8(&bytes[..end]).is_ok() {
            return end;
        }
    }
    0
}

fn is_utf8_continuation(b: u8) -> bool {
    (b & 0b1100_0000) == 0b1000_0000
}

fn drain_pipe(pipe: &mut impl Read, head_cap: usize, tail_cap: usize) -> String {
    let mut clipper = StreamClipper::new(head_cap, tail_cap);
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => clipper.push(&buf[..n]),
            Err(_) => break,
        }
    }
    clipper.finish()
}

#[cfg(unix)]
fn spawn_command(
    root: &Path,
    program: &str,
    args: &[String],
) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt as _;
    Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
}

#[cfg(not(unix))]
fn spawn_command(
    root: &Path,
    program: &str,
    args: &[String],
) -> std::io::Result<std::process::Child> {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

#[cfg(unix)]
fn kill_process_group_pgid(pgid: i32) {
    unsafe {
        let rc = libc::killpg(pgid, libc::SIGKILL);
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                let _ = err;
            }
        }
    }
}

#[cfg(unix)]
fn kill_process_group(child: &mut std::process::Child) {
    let pid = child.id() as i32;
    kill_process_group_pgid(pid);
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut std::process::Child) {
    let _ = child.kill();
}

fn join_readers(
    child: &mut std::process::Child,
    out_reader: std::thread::JoinHandle<String>,
    err_reader: std::thread::JoinHandle<String>,
) -> (String, String) {
    kill_process_group(child);
    let _ = child.wait();
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    (stdout, stderr)
}

fn finish_timeout(
    child: &mut std::process::Child,
    out_reader: std::thread::JoinHandle<String>,
    err_reader: std::thread::JoinHandle<String>,
    started: Instant,
) -> RunHit {
    let (stdout, stderr) = join_readers(child, out_reader, err_reader);
    RunHit {
        code: -1,
        stdout,
        stderr,
        timed_out: true,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

/// Run `program` with `args` in `root`, killing after `timeout_secs`.
///
/// Output is head+tail clipped per stream using a 2:6 split of `output_cap`.
/// On timeout the process group is killed and a [`RunHit`] is returned with
/// `timed_out: true`, `code: -1`, and output captured so far.
///
/// # Errors
///
/// Returns [`Error::EmptyProgram`] for empty programs and [`Error::Io`] when
/// spawning fails. A nonzero exit is a normal [`RunHit`] with `timed_out: false`.
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
    let (head_cap, tail_cap) = head_tail_caps(output_cap);
    let started = Instant::now();
    let mut child = spawn_command(root, program, args)?;
    #[cfg(unix)]
    let pgid = child.id() as i32;
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(pipe) = out_pipe.as_mut() {
            s = drain_pipe(pipe, head_cap, tail_cap);
        }
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(pipe) = err_pipe.as_mut() {
            s = drain_pipe(pipe, head_cap, tail_cap);
        }
        s
    });
    let deadline = Duration::from_secs(timeout_secs.max(1));
    loop {
        match child.try_wait()? {
            Some(status) => {
                #[cfg(unix)]
                kill_process_group_pgid(pgid);
                #[cfg(not(unix))]
                kill_process_group(&mut child);
                let stdout = out_reader.join().unwrap_or_default();
                let stderr = err_reader.join().unwrap_or_default();
                return Ok(RunHit {
                    code: status.code().unwrap_or(-1),
                    stdout,
                    stderr,
                    timed_out: false,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                });
            }
            None if started.elapsed() >= deadline => {
                return Ok(finish_timeout(&mut child, out_reader, err_reader, started));
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn head_tail_caps_default_ratio() {
        let (h, t) = head_tail_caps(OUTPUT_CAP_BYTES);
        assert_eq!(h, OUTPUT_HEAD_BYTES);
        assert_eq!(t, OUTPUT_TAIL_BYTES);
    }

    #[test]
    fn echo_returns_output_and_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hit = run(dir.path(), "echo", &["hi".to_owned()], 10, OUTPUT_CAP_BYTES).expect("run");
        assert_eq!(hit.code, 0);
        assert!(!hit.timed_out);
        assert_eq!(hit.stdout.trim_end(), "hi");
    }

    #[test]
    fn small_ascii_output_byte_identical() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hit = run(
            dir.path(),
            "printf",
            &["%s".to_owned(), "hello-world".to_owned()],
            10,
            OUTPUT_CAP_BYTES,
        )
        .expect("run");
        assert_eq!(hit.stdout, "hello-world");
    }

    #[test]
    fn in_budget_multibyte_tail_intact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hit = run(
            dir.path(),
            "printf",
            &["%s".to_owned(), "abcα".to_owned()],
            10,
            OUTPUT_CAP_BYTES,
        )
        .expect("run");
        assert_eq!(hit.stdout, "abcα");
        assert!(!hit.stdout.contains('\u{FFFD}'));
    }

    #[test]
    fn multibyte_straddling_head_tail_cuts() {
        let mut c = StreamClipper::new(4, 4);
        c.push("αβ".as_bytes());
        c.push("γδεζη".as_bytes());
        let s = c.finish();
        assert!(!s.contains('\u{FFFD}'));
        assert!(s.contains("bytes elided"));
    }

    #[test]
    fn bash_background_sleep_does_not_block_join() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = "toolgate_pgrep_marker_884422";
        let script = format!("sleep 99999 {marker} & echo hi");
        let started = Instant::now();
        let hit = run(
            dir.path(),
            "bash",
            &["-c".to_owned(), script],
            10,
            OUTPUT_CAP_BYTES,
        )
        .expect("run");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(hit.stdout.trim(), "hi");
        std::thread::sleep(Duration::from_millis(300));
        let probe = Command::new("pgrep").args(["-f", marker]).output();
        let alive = probe.map(|o| !o.stdout.is_empty()).unwrap_or(false);
        assert!(!alive, "background sleep still alive");
    }

    #[test]
    fn nonzero_exit_is_data_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hit = run(dir.path(), "false", &[], 10, OUTPUT_CAP_BYTES).expect("run");
        assert_eq!(hit.code, 1);
        assert!(!hit.timed_out);
    }

    #[test]
    fn timeout_kills_and_returns_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hit = run(dir.path(), "sleep", &["30".to_owned()], 1, OUTPUT_CAP_BYTES)
            .expect("must time out with hit");
        assert!(hit.timed_out);
        assert_eq!(hit.code, -1);
        assert!(hit.elapsed_ms < 10_000);
    }

    #[test]
    fn timeout_kills_grandchildren() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = "sleep 99999 & sleep 99999";
        let started = Instant::now();
        let hit = run(
            dir.path(),
            "bash",
            &["-c".to_owned(), script.to_owned()],
            1,
            OUTPUT_CAP_BYTES,
        )
        .expect("run");
        assert!(hit.timed_out);
        assert!(started.elapsed() < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(300));
        let probe = Command::new("pgrep").args(["-f", "sleep 99999"]).output();
        let alive = probe.map(|o| !o.stdout.is_empty()).unwrap_or(false);
        assert!(!alive, "grandchild sleep processes still alive");
    }

    #[test]
    fn big_output_is_head_tail_clipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hit = run(
            dir.path(),
            "bash",
            &["-c".to_owned(), "yes | head -c 200000".to_owned()],
            10,
            1024,
        )
        .expect("run");
        assert!(hit.stdout.len() <= 1200);
        assert!(hit.stdout.contains("bytes elided"));
    }

    #[test]
    fn empty_program_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = run(dir.path(), "  ", &[], 10, OUTPUT_CAP_BYTES).expect_err("empty");
        assert!(matches!(err, Error::EmptyProgram));
    }

    #[test]
    fn clipper_char_boundary_safe() {
        let mut c = StreamClipper::new(4, 4);
        c.push("αβγδ".as_bytes());
        c.push("εζηθ".as_bytes());
        c.push("ι".as_bytes());
        let s = c.finish();
        assert!(s.is_char_boundary(s.len()));
    }

    fn elided_count(output: &str) -> usize {
        let rest = output.split("... [").nth(1).expect("elision marker");
        let mid = rest.split("] ...").next().expect("elision close");
        mid.strip_suffix(" bytes elided")
            .expect("elided suffix")
            .parse()
            .expect("elided number")
    }

    fn assert_clip_no_fffd_and_elided(
        head_cap: usize,
        tail_cap: usize,
        prefix: &str,
        ch: char,
        filler: &[u8],
        suffix: &str,
    ) {
        assert!(!prefix.contains('\u{FFFD}'));
        assert!(!suffix.contains('\u{FFFD}'));
        let mut body = prefix.to_owned();
        body.push(ch);
        body.push_str(&"z".repeat(filler.len()));
        body.push_str(suffix);
        let total = body.as_bytes().len();
        let mut c = StreamClipper::new(head_cap, tail_cap);
        c.push(body.as_bytes());
        let out = c.finish();
        assert!(
            !out.contains('\u{FFFD}'),
            "output had replacement char for scalar {ch:?}: {out:?}"
        );
        assert!(out.contains("bytes elided"));
        let (head_part, tail_and_mid) = out.split_once("... [").expect("split head");
        let (mid, tail_part) = tail_and_mid.split_once("] ...").expect("split tail");
        let _ = mid;
        let head_kept = head_part.as_bytes().len();
        let tail_kept = tail_part.as_bytes().len();
        assert_eq!(
            elided_count(&out),
            total - head_kept - tail_kept,
            "elided byte count for {ch:?}"
        );
    }

    #[test]
    fn clipper_truncates_inside_multibyte_scalars_without_fffd() {
        // Head cap ends inside the lead byte / continuation of 2-, 3-, and 4-byte UTF-8.
        assert_clip_no_fffd_and_elided(3, 4, "aa", 'α', b"bbbbbbbbbb", "cc");
        assert_clip_no_fffd_and_elided(3, 4, "aa", '中', b"bbbbbbbbbb", "cc");
        assert_clip_no_fffd_and_elided(3, 4, "aa", '🎉', b"bbbbbbbbbb", "cc");
    }

    fn clip_parts(out: &str) -> (usize, usize, usize) {
        if !out.contains("bytes elided") {
            let kept = out.as_bytes().len();
            return (kept, 0, 0);
        }
        let (head_part, tail_and_mid) = out.split_once("... [").expect("split head");
        let (mid, tail_part) = tail_and_mid.split_once("] ...").expect("split tail");
        let elided = mid
            .strip_suffix(" bytes elided")
            .expect("elided suffix")
            .parse()
            .expect("elided number");
        (
            head_part.as_bytes().len(),
            tail_part.as_bytes().len(),
            elided,
        )
    }

    #[test]
    fn clipper_property_caps_and_scalar_splits() {
        const SCALARS: &[char] = &['a', 'α', '中', '🎉', 'z'];
        let mut seq = String::new();
        for _ in 0..6 {
            for ch in SCALARS {
                seq.push(*ch);
            }
        }
        for split in 0..=seq.len() {
            let body = format!(
                "{}{}",
                seq.get(..split).unwrap_or(""),
                seq.get(split..).unwrap_or("")
            );
            let total = body.as_bytes().len();
            for cap in 0..=64 {
                let (head_cap, tail_cap) = head_tail_caps(cap);
                let mut c = StreamClipper::new(head_cap, tail_cap);
                c.push(body.as_bytes());
                let out = c.finish();
                assert!(
                    !out.contains('\u{FFFD}'),
                    "cap={cap} split={split} body_len={total}: {out:?}"
                );
                let (head_kept, tail_kept, elided) = clip_parts(&out);
                assert_eq!(
                    head_kept + tail_kept + elided,
                    total,
                    "cap={cap} split={split}"
                );
            }
        }
    }
}
