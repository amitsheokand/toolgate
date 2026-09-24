//! Exact-string file edits that return a short diff.
//!
//! The point (reads track, item 4): after an edit, the agent gets the diff
//! back instead of re-reading the file to check its own write. Fail closed:
//! the match must be unique unless `--all` is passed, and the returned
//! diff is capped.

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

/// Max diff lines returned per edit (head-truncated with a marker).
pub const DIFF_CAP_LINES: usize = 60;

/// Edit errors: caller-visible, never dumps unrelated file regions.
#[derive(Debug, Error)]
pub enum Error {
    /// Propagated read-gate errors (escape, binary, missing file).
    #[error(transparent)]
    Read(#[from] crate::read::Error),
    /// Old text not found, or found more than once without `--all`.
    #[error("{0}")]
    NoMatch(String),
}

/// One applied edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditHit {
    /// Absolute file path.
    pub path: std::path::PathBuf,
    /// 1-based first changed line.
    pub start: u64,
    /// 1-based last changed line (inclusive, in the *new* file).
    pub end: u64,
    /// Replacements applied.
    pub applied: usize,
    /// Unified-style diff, capped at [`DIFF_CAP_LINES`] lines.
    pub diff: Vec<String>,
}

/// Replace `old` with `new` in the file at `raw` under `root`.
///
/// Exactly one occurrence must match unless `all` is set (then every
/// occurrence is replaced). Returns the bounded diff so the caller does
/// not re-read the file.
///
/// # Errors
///
/// Returns [`Error`] for read-gate failures (escape, binary, missing) or
/// when the match is missing/ambiguous.
pub fn edit(root: &Path, raw: &str, old: &str, new: &str, all: bool) -> Result<EditHit, Error> {
    if old.is_empty() {
        return Err(Error::NoMatch("old text must not be empty".to_owned()));
    }
    // Reuse the read gate's resolution + binary guards by reading whole
    // through a large limit: refusal semantics stay in one place.
    let probe = crate::read::read(root, raw, None, 0, None, u64::MAX)?;
    let occurrences = probe.text.join("\n").matches(old).count();
    if occurrences == 0 {
        return Err(Error::NoMatch("old text not found".to_owned()));
    }
    if occurrences > 1 && !all {
        return Err(Error::NoMatch(format!(
            "old text matches {occurrences} times; pass --all or narrow it"
        )));
    }
    let path = probe.path.clone();
    let before = probe.text.join("\n");
    let updated = before.replacen(old, new, if all { usize::MAX } else { 1 });
    // Preserve the trailing-newline shape of the original file.
    let raw_bytes = std::fs::read(&path).map_err(crate::read::Error::from)?;
    let trailing_nl = raw_bytes.last().is_some_and(|b| *b == b'\n');
    let mut out = updated.clone();
    if trailing_nl {
        out.push('\n');
    }
    std::fs::write(&path, out).map_err(crate::read::Error::from)?;
    let applied = if all { occurrences } else { 1 };
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = updated.lines().collect();
    let (first, last_b, last_a) = change_span(&before_lines, &after_lines);
    Ok(EditHit {
        // 1-based hunk span in the new file.
        start: first as u64 + 1,
        end: last_a as u64,
        path,
        applied,
        diff: bounded_diff(&before_lines, &after_lines, first, last_b, last_a),
    })
}

/// 0-based [first, last) span covering all differing lines.
fn change_span(before: &[&str], after: &[&str]) -> (usize, usize, usize) {
    let mut first = 0;
    while first < before.len() && first < after.len() && before[first] == after[first] {
        first += 1;
    }
    let mut last_b = before.len();
    let mut last_a = after.len();
    while last_b > first && last_a > first && before[last_b - 1] == after[last_a - 1] {
        last_b -= 1;
        last_a -= 1;
    }
    (first, last_b, last_a)
}

/// Minimal line diff around the changed region, capped.
fn bounded_diff(
    before: &[&str],
    after: &[&str],
    first: usize,
    last_b: usize,
    last_a: usize,
) -> Vec<String> {
    let mut out = vec![format!(
        "@@ -{},{} +{},{} @@",
        first + 1,
        last_b - first,
        first + 1,
        last_a - first
    )];
    for line in &before[first..last_b] {
        out.push(format!("-{line}"));
    }
    for line in &after[first..last_a] {
        out.push(format!("+{line}"));
    }
    if out.len() > DIFF_CAP_LINES {
        out.truncate(DIFF_CAP_LINES);
        out.push("... (diff truncated)".to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, contents) in files {
            let path = dir.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("mkdir");
            }
            std::fs::write(&path, contents).expect("write");
        }
        dir
    }

    #[test]
    fn edit_replaces_once_and_returns_diff() {
        let dir = workspace_with(&[("a.txt", "alpha\nbeta\ngamma\n")]);
        let hit = edit(dir.path(), "a.txt", "beta", "BETA", false).expect("edit");
        assert_eq!(hit.applied, 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).expect("read"),
            "alpha\nBETA\ngamma\n"
        );
        assert!(hit.diff.iter().any(|l| l == "-beta"));
        assert!(hit.diff.iter().any(|l| l == "+BETA"));
    }

    #[test]
    fn ambiguous_match_needs_all() {
        let dir = workspace_with(&[("a.txt", "x\nx\nx\n")]);
        let err = edit(dir.path(), "a.txt", "x", "y", false).expect_err("ambiguous");
        assert!(matches!(err, Error::NoMatch(_)));
        let hit = edit(dir.path(), "a.txt", "x", "y", true).expect("edit");
        assert_eq!(hit.applied, 3);
    }

    #[test]
    fn missing_match_is_an_error_not_empty_diff() {
        let dir = workspace_with(&[("a.txt", "alpha\n")]);
        let err = edit(dir.path(), "a.txt", "zzz", "y", false).expect_err("missing");
        assert!(matches!(err, Error::NoMatch(_)));
    }

    #[test]
    fn escapes_and_binaries_refused() {
        let dir = workspace_with(&[("a.txt", "hi\n")]);
        let err = edit(dir.path(), "../e.txt", "hi", "yo", false).expect_err("escape");
        assert!(matches!(err, Error::Read(_)));
        std::fs::write(dir.path().join("b.bin"), [0x00]).expect("write");
        let err = edit(dir.path(), "b.bin", "x", "y", false).expect_err("binary");
        assert!(matches!(err, Error::Read(_)));
    }
}
