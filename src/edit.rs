//! Exact-string file edits that return a short diff.
//!
//! The point (reads track, item 4): after an edit, the agent gets the diff
//! back instead of re-reading the file to check its own write. Fail closed:
//! the match must be unique unless `--all` is passed, and the returned
//! diff is capped.

use std::fs::{self, Permissions};
use std::path::Path;

use serde::Serialize;
use thiserror::Error;

/// Max diff lines returned per edit (head-truncated with a marker).
pub const DIFF_CAP_LINES: usize = 60;
/// Context lines on each side of the hunk in the returned diff.
const DIFF_CONTEXT_LINES: usize = 3;

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
    let loaded = crate::read::load_file(root, raw, true)?;
    let path = loaded.path;
    let before = loaded.text;
    let occurrences = before.matches(old).count();
    if occurrences == 0 {
        return Err(Error::NoMatch("old text not found".to_owned()));
    }
    if occurrences > 1 && !all {
        return Err(Error::NoMatch(format!(
            "old text matches {occurrences} times; pass --all or narrow it"
        )));
    }
    let updated = if all {
        before.replace(old, new)
    } else {
        before.replacen(old, new, 1)
    };
    let perms = fs::metadata(&path)
        .map_err(crate::read::Error::from)?
        .permissions();
    atomic_write(&path, updated.as_bytes(), &perms)?;
    let applied = if all { occurrences } else { 1 };
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = updated.lines().collect();
    let (first, last_b, last_a) = change_span(&before_lines, &after_lines);
    Ok(EditHit {
        start: first as u64 + 1,
        end: last_a as u64,
        path,
        applied,
        diff: bounded_diff(&before_lines, &after_lines, first, last_b, last_a),
    })
}

fn atomic_write(path: &Path, bytes: &[u8], perms: &Permissions) -> Result<(), crate::read::Error> {
    let parent = path.parent().ok_or_else(|| {
        crate::read::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no parent",
        ))
    })?;
    let tmp = parent.join(format!(".toolgate-{}", std::process::id()));
    fs::write(&tmp, bytes).map_err(crate::read::Error::from)?;
    fs::set_permissions(&tmp, perms.clone()).map_err(crate::read::Error::from)?;
    fs::rename(&tmp, path).map_err(crate::read::Error::from)?;
    Ok(())
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

/// Unified-style diff around the changed region with context, capped.
fn bounded_diff(
    before: &[&str],
    after: &[&str],
    first: usize,
    last_b: usize,
    last_a: usize,
) -> Vec<String> {
    let ctx_start = first.saturating_sub(DIFF_CONTEXT_LINES);
    let ctx_end_b = (last_b + DIFF_CONTEXT_LINES).min(before.len());
    let ctx_end_a = (last_a + DIFF_CONTEXT_LINES).min(after.len());
    let old_count = ctx_end_b - ctx_start;
    let new_count = ctx_end_a - ctx_start;
    let mut out = vec![format!(
        "@@ -{},{} +{},{} @@",
        ctx_start + 1,
        old_count,
        ctx_start + 1,
        new_count
    )];
    for i in ctx_start..first {
        out.push(format!(" {}", before[i]));
    }
    for i in first..last_b {
        out.push(format!("-{}", before[i]));
    }
    for i in first..last_a {
        out.push(format!("+{}", after[i]));
    }
    for i in last_b..ctx_end_b {
        out.push(format!(" {}", before[i]));
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
    use std::os::unix::fs::{PermissionsExt, symlink};

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
    fn crlf_preserved_on_edit() {
        let dir = workspace_with(&[("a.txt", "alpha\r\nbeta\r\ngamma\r\n")]);
        let hit = edit(dir.path(), "a.txt", "beta", "BETA", false).expect("edit");
        assert_eq!(hit.applied, 1);
        let bytes = std::fs::read(dir.path().join("a.txt")).expect("read");
        assert_eq!(
            bytes, b"alpha\r\nBETA\r\ngamma\r\n",
            "CRLF must stay byte-for-byte"
        );
        edit(dir.path(), "a.txt", "BETA", "beta", false).expect("revert");
        let bytes = std::fs::read(dir.path().join("a.txt")).expect("read");
        assert_eq!(bytes, b"alpha\r\nbeta\r\ngamma\r\n");
    }

    #[test]
    fn crlf_old_string_matches() {
        let dir = workspace_with(&[("a.txt", "x\r\ny\r\n")]);
        edit(dir.path(), "a.txt", "x\r\ny", "z", false).expect("edit");
        assert_eq!(
            std::fs::read(dir.path().join("a.txt")).expect("read"),
            b"z\r\n"
        );
    }

    #[test]
    fn non_utf8_refused_and_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.bin");
        std::fs::write(&path, [0xff, 0xfe, b'a']).expect("write");
        let before = std::fs::read(&path).expect("read");
        let err = edit(dir.path(), "a.bin", "a", "b", false).expect_err("utf8");
        assert!(matches!(err, Error::Read(crate::read::Error::NotUtf8(_))));
        assert_eq!(std::fs::read(&path).expect("read"), before);
    }

    #[test]
    fn permissions_preserved() {
        let dir = workspace_with(&[("a.txt", "data\n")]);
        let path = dir.path().join("a.txt");
        let mut perms = std::fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&path, perms).expect("chmod");
        edit(dir.path(), "a.txt", "data", "DATA", false).expect("edit");
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn diff_includes_context_lines() {
        let dir = workspace_with(&[("a.txt", "c1\nc2\nc3\nbefore\nafter\nc4\nc5\nc6\n")]);
        let hit = edit(dir.path(), "a.txt", "before", "BEFORE", false).expect("edit");
        let ctx = hit
            .diff
            .iter()
            .filter(|l| l.starts_with(' ') && !l.starts_with("@@"))
            .count();
        assert!(ctx >= 2, "expected context lines in diff: {:?}", hit.diff);
        assert!(hit.diff[0].starts_with("@@"));
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
    fn symlink_outside_root_refused_edit() {
        let outer = tempfile::tempdir().expect("outer");
        std::fs::write(outer.path().join("secret.txt"), "nope\n").expect("write");
        let dir = tempfile::tempdir().expect("inner");
        symlink(outer.path().join("secret.txt"), dir.path().join("link.txt")).expect("symlink");
        let err = edit(dir.path(), "link.txt", "nope", "x", false).expect_err("escape");
        assert!(matches!(err, Error::Read(crate::read::Error::Escape(_))));
    }

    #[test]
    fn symlink_inside_root_allowed_edit() {
        let dir = workspace_with(&[("real.txt", "ok\n")]);
        symlink(dir.path().join("real.txt"), dir.path().join("link.txt")).expect("symlink");
        edit(dir.path(), "link.txt", "ok", "yes", false).expect("edit");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("link.txt")).expect("read"),
            "yes\n"
        );
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
