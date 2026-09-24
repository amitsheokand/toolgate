//! Capped file reads: the anti-whole-file-read gate.
//!
//! Rules (fail closed):
//! - `line` given (1-based): return a `radius`-line window around it
//!   (default 100 each way → ~200 lines), clamped to the file. Never the
//!   whole file regardless of size.
//! - No `line`: files at or under [`WHOLE_FILE_LIMIT_LINES`] lines read
//!   whole; larger files require an explicit `--start/--end` range (or a
//!   `line`). The error names the file's line count and the flags to use.
//! - Binary files (NUL byte) and unreadable paths are errors, not dumps.
//! - Paths escaping the workspace root are rejected.

use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

/// Default window radius around `line` (→ ~200-line windows).
pub const DEFAULT_RADIUS: u64 = 100;
/// Files longer than this need an explicit range (or `line`) to read.
pub const WHOLE_FILE_LIMIT_LINES: u64 = 400;
/// Lines longer than this mark a file as generated/data: refused.
const MAX_LINE_CHARS: usize = 8192;

/// Read errors: all caller-visible, none dump file contents.
#[derive(Debug, Error)]
pub enum Error {
    /// Path escapes the workspace root.
    #[error("path escapes workspace root: {0}")]
    Escape(String),
    /// Filesystem or workspace access failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Whole-file read requires an explicit range (file too large).
    #[error("refusing whole-file read of {lines} lines; pass --start/--end or --line")]
    RangeRequired {
        /// Total lines in the file.
        lines: u64,
    },
    /// Binary content refused.
    #[error("refusing binary file: {0}")]
    Binary(String),
}

/// One bounded read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadHit {
    /// Absolute file path.
    pub path: PathBuf,
    /// 1-based first line returned.
    pub start: u64,
    /// 1-based last line returned (inclusive).
    pub end: u64,
    /// Total lines in the file.
    pub total: u64,
    /// Returned line texts (no trailing newline).
    pub text: Vec<String>,
}

fn path_under_canonical_root(path: &Path, canon_root: &Path) -> bool {
    path.starts_with(canon_root)
}

/// Join `value` to `root`, walking one component at a time: canonicalize each
/// existing prefix (symlinks resolved) before the next step; `..` applies to
/// the resolved directory. Non-existent suffixes are lexical only (no `..`).
#[must_use]
pub fn inside_root(root: impl AsRef<Path>, value: &str) -> Option<PathBuf> {
    let root = root.as_ref();
    let canon_root = root.canonicalize().ok()?;
    let value_path = Path::new(value);

    let components: Vec<std::path::Component<'_>> = if value_path.is_absolute() {
        if !value_path.starts_with(root) {
            return None;
        }
        value_path.strip_prefix(root).ok()?.components().collect()
    } else {
        value_path.components().collect()
    };

    let mut current = canon_root.clone();
    let mut lexical_tail = false;

    for component in components {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {}
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if lexical_tail {
                    return None;
                }
                if !current.pop() {
                    return None;
                }
                if !path_under_canonical_root(&current, &canon_root) {
                    return None;
                }
                if current.exists() {
                    current = current.canonicalize().ok()?;
                    if !path_under_canonical_root(&current, &canon_root) {
                        return None;
                    }
                    lexical_tail = false;
                }
            }
            std::path::Component::Normal(part) => {
                current.push(part);
                if current.exists() {
                    current = current.canonicalize().ok()?;
                    if !path_under_canonical_root(&current, &canon_root) {
                        return None;
                    }
                    lexical_tail = false;
                } else {
                    lexical_tail = true;
                }
            }
        }
    }

    if path_under_canonical_root(&current, &canon_root) {
        Some(current)
    } else {
        None
    }
}

/// Resolve `raw` against `root`, rejecting escapes.
fn resolve(root: &Path, raw: &str) -> Result<PathBuf, Error> {
    inside_root(root, raw).ok_or_else(|| Error::Escape(raw.to_owned()))
}

/// Read `lines` (1-based, inclusive) from already-loaded text.
fn slice_lines(text: &str, start: u64, end: u64) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(i, _)| (*i as u64) + 1 >= start && (*i as u64) + 1 <= end)
        .map(|(_, l)| l.to_owned())
        .collect()
}

/// Bounded read of `raw` under `root`.
///
/// - `line` (`Some`, 1-based) wins over `range`: window
///   `[line-radius, line+radius]` clamped to the file.
/// - `range` (`Some((start, end))`, 1-based inclusive): exactly that.
/// - Neither: whole file iff `total <= limit_lines`, else
///   [`Error::RangeRequired`].
///
/// # Errors
///
/// Returns [`Error`] for escapes, missing/unreadable/binary files, bad
/// ranges, or oversized whole-file reads.
pub fn read(
    root: &Path,
    raw: &str,
    line: Option<u64>,
    radius: u64,
    range: Option<(u64, u64)>,
    limit_lines: u64,
) -> Result<ReadHit, Error> {
    let path = resolve(root, raw)?;
    let bytes = std::fs::read(&path)?;
    if bytes.contains(&0) {
        return Err(Error::Binary(path.display().to_string()));
    }
    let text = String::from_utf8_lossy(&bytes);
    if text.lines().any(|l| l.len() > MAX_LINE_CHARS) {
        return Err(Error::Binary(format!(
            "{}: generated/data (overlong lines)",
            path.display()
        )));
    }
    let total = text.lines().count() as u64;
    if total == 0 {
        return Ok(ReadHit {
            path,
            start: 1,
            end: 0,
            total: 0,
            text: Vec::new(),
        });
    }
    let (start, end) = if let Some(line) = line {
        if line < 1 || line > total {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("line {line} out of range 1..={total}"),
            )));
        }
        (
            line.saturating_sub(radius).max(1),
            (line + radius).min(total),
        )
    } else if let Some((start, end)) = range {
        if start < 1 || end < start || end > total {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("range {start}-{end} out of 1..={total}"),
            )));
        }
        (start, end)
    } else if total <= limit_lines {
        (1, total)
    } else {
        return Err(Error::RangeRequired { lines: total });
    };
    Ok(ReadHit {
        path,
        start,
        end,
        total,
        text: slice_lines(&text, start, end),
    })
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

    fn numbered(n: u64) -> String {
        (1..=n)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[test]
    fn line_returns_window_clamped_to_file() {
        let dir = workspace_with(&[("a.txt", &numbered(500))]);
        let hit = read(
            dir.path(),
            "a.txt",
            Some(250),
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("read");
        assert_eq!((hit.start, hit.end, hit.total), (150, 350, 500));
        assert_eq!(hit.text.len(), 201);
        // Edges clamp instead of failing.
        let top = read(
            dir.path(),
            "a.txt",
            Some(1),
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("read");
        assert_eq!((top.start, top.end), (1, 101));
        let bottom = read(
            dir.path(),
            "a.txt",
            Some(500),
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("read");
        assert_eq!((bottom.start, bottom.end), (400, 500));
    }

    #[test]
    fn small_files_read_whole_large_need_range() {
        let dir = workspace_with(&[("small.txt", &numbered(100)), ("big.txt", &numbered(500))]);
        let hit = read(
            dir.path(),
            "small.txt",
            None,
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("read");
        assert_eq!((hit.start, hit.end), (1, 100));
        let err = read(
            dir.path(),
            "big.txt",
            None,
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect_err("must refuse");
        assert!(matches!(err, Error::RangeRequired { lines: 500 }));
        // Explicit range always works.
        let hit = read(
            dir.path(),
            "big.txt",
            None,
            100,
            Some((10, 20)),
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("read");
        assert_eq!((hit.start, hit.end), (10, 20));
        assert_eq!(hit.text.len(), 11);
    }

    #[test]
    fn inside_root_rejects_lexical_escape() {
        let dir = workspace_with(&[("a.txt", "hi\n")]);
        assert!(inside_root(dir.path(), "../escape").is_none());
        assert!(inside_root(dir.path(), "sub/../../outside").is_none());
        assert!(inside_root(dir.path(), "a.txt").is_some());
    }

    #[test]
    fn inside_root_rejects_symlink_component_walk() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let root = root_dir.path();
        let outside = tempfile::tempdir().expect("outside");
        std::fs::create_dir(root.join("a")).expect("mkdir a");
        std::os::unix::fs::symlink(outside.path(), root.join("link")).expect("symlink");
        for value in ["link/nested", "link/../out", "link/../../x", "a/../link/x"] {
            assert!(inside_root(root, value).is_none(), "{value}");
        }
    }

    #[test]
    fn escapes_and_binaries_refused() {
        let dir = workspace_with(&[("a.txt", "hi\n")]);
        let err = read(
            dir.path(),
            "../escape.txt",
            None,
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect_err("escape");
        assert!(matches!(err, Error::Escape(_)));
        std::fs::write(dir.path().join("b.bin"), [0x00, 0x01]).expect("write");
        let err =
            read(dir.path(), "b.bin", None, 100, None, WHOLE_FILE_LIMIT_LINES).expect_err("binary");
        assert!(matches!(err, Error::Binary(_)));
        let err = read(
            dir.path(),
            "a.txt",
            Some(99),
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect_err("range");
        assert!(matches!(err, Error::Io(_)));
    }
}
