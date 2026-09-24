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
//! - Paths escaping the workspace root (including via symlinks) are rejected.

use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

/// Default window radius around `line` (→ ~200-line windows).
pub const DEFAULT_RADIUS: u64 = 100;
/// Hard cap on window radius (values above are clamped).
pub const MAX_RADIUS: u64 = 200;
/// Files longer than this need an explicit range (or `line`) to read.
pub const WHOLE_FILE_LIMIT_LINES: u64 = 400;
/// Max formatted read output size (chars); stops at the last whole line.
pub const READ_CHAR_BUDGET: usize = 48_000;
/// Lines longer than this are truncated in read output (not refused).
pub const MAX_LINE_CHARS: usize = 8192;

const TRUNCATED_LINE_MARKER: &str = " ... (line truncated)";

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
    /// Explicit range spans more lines than allowed.
    #[error("refusing read range of {span} lines; max is {max} lines per request")]
    RangeTooLarge {
        /// Requested span.
        span: u64,
        /// Maximum allowed span.
        max: u64,
    },
    /// Binary content refused.
    #[error("refusing binary file: {0}")]
    Binary(String),
    /// Strict UTF-8 required (edit path).
    #[error("refusing non-UTF-8 file: {0}")]
    NotUtf8(String),
    /// Generated/data file (overlong lines) refused on edit.
    #[error("refusing generated/data file: {0}")]
    Generated(String),
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

/// Lexically normalize `raw` against `root` (no symlink follow).
fn resolve_lexical(root: &Path, raw: &str) -> Result<PathBuf, Error> {
    let joined = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };
    let mut normal = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::Prefix(prefix) => normal.push(prefix.as_os_str()),
            std::path::Component::RootDir => normal.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normal.pop();
            }
            std::path::Component::Normal(part) => normal.push(part),
        }
    }
    if normal.starts_with(root) {
        Ok(normal)
    } else {
        Err(Error::Escape(raw.to_owned()))
    }
}

/// Resolve `raw` under `root`, rejecting lexical and symlink escapes.
pub(crate) fn resolve_path(root: &Path, raw: &str) -> Result<PathBuf, Error> {
    let path = resolve_lexical(root, raw)?;
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical_target = if path.exists() {
        std::fs::canonicalize(&path)?
    } else {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let canonical_parent = if parent.exists() {
            std::fs::canonicalize(parent)?
        } else {
            let parent_lex = resolve_lexical(root, parent.to_string_lossy().as_ref())?;
            std::fs::canonicalize(&parent_lex)?
        };
        let name = path.file_name().ok_or_else(|| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path has no file name",
            ))
        })?;
        canonical_parent.join(name)
    };
    if canonical_target.starts_with(&canonical_root) {
        Ok(path)
    } else {
        Err(Error::Escape(raw.to_owned()))
    }
}

/// Loaded file text for read/edit gates.
pub(crate) struct LoadedFile {
    pub path: PathBuf,
    pub text: String,
    pub total_lines: u64,
}

/// Load bytes under the path gate; optional generated-file refusal (edit).
pub(crate) fn load_file(
    root: &Path,
    raw: &str,
    refuse_generated: bool,
) -> Result<LoadedFile, Error> {
    let path = resolve_path(root, raw)?;
    let bytes = std::fs::read(&path)?;
    if bytes.contains(&0) {
        return Err(Error::Binary(path.display().to_string()));
    }
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return Err(Error::NotUtf8(path.display().to_string())),
    };
    if refuse_generated && text.lines().any(|l| l.len() > MAX_LINE_CHARS) {
        return Err(Error::Generated(path.display().to_string()));
    }
    let total_lines = count_lines(&text);
    Ok(LoadedFile {
        path,
        text,
        total_lines,
    })
}

fn count_lines(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    text.lines().count() as u64
}

/// Read `lines` (1-based, inclusive) from already-loaded text.
fn slice_lines(text: &str, start: u64, end: u64) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(i, _)| (*i as u64) + 1 >= start && (*i as u64) + 1 <= end)
        .map(|(_, l)| truncate_line_display(l))
        .collect()
}

fn truncate_line_display(line: &str) -> String {
    if line.len() <= MAX_LINE_CHARS {
        line.to_owned()
    } else {
        let mut s = line[..MAX_LINE_CHARS].to_owned();
        s.push_str(TRUNCATED_LINE_MARKER);
        s
    }
}

/// Prefix each line with a 1-based line number, right-aligned, then `|`.
pub fn format_numbered_lines(start: u64, lines: &[String]) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    let last = start + lines.len() as u64 - 1;
    let width = last.to_string().len();
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let n = start + i as u64;
            format!("{n:>width$}|{line}", n = n, line = line, width = width)
        })
        .collect()
}

/// Shrink `end` so formatted lines fit in [`READ_CHAR_BUDGET`].
pub fn apply_char_budget(start: u64, end: u64, lines: Vec<String>) -> (u64, Vec<String>) {
    if lines.is_empty() {
        return (end, lines);
    }
    let mut budget = READ_CHAR_BUDGET;
    let mut kept = Vec::new();
    let mut last_line = start;
    for (i, line) in lines.into_iter().enumerate() {
        let n = start + i as u64;
        let width = end.to_string().len().max(n.to_string().len());
        let row_len = format!("{n:>width$}|{line}", n = n, line = line, width = width).len() + 1;
        if kept.is_empty() || budget >= row_len {
            budget = budget.saturating_sub(row_len);
            kept.push(line);
            last_line = n;
        } else {
            break;
        }
    }
    (last_line, kept)
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
    let radius = radius.min(MAX_RADIUS);
    let loaded = load_file(root, raw, false)?;
    let path = loaded.path;
    let text = loaded.text;
    let total = loaded.total_lines;
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
        let span = end - start + 1;
        if span > limit_lines {
            return Err(Error::RangeTooLarge {
                span,
                max: limit_lines,
            });
        }
        (start, end)
    } else if total <= limit_lines {
        (1, total)
    } else {
        return Err(Error::RangeRequired { lines: total });
    };
    let lines = slice_lines(&text, start, end);
    let (end, text) = apply_char_budget(start, end, lines);
    Ok(ReadHit {
        path,
        start,
        end,
        total,
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

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
    fn radius_clamped_to_max() {
        let dir = workspace_with(&[("a.txt", &numbered(1000))]);
        let hit = read(
            dir.path(),
            "a.txt",
            Some(500),
            500,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("read");
        assert_eq!((hit.start, hit.end), (300, 700));
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
    fn range_over_limit_refused() {
        let dir = workspace_with(&[("a.txt", &numbered(500))]);
        let err = read(
            dir.path(),
            "a.txt",
            None,
            100,
            Some((1, 401)),
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect_err("span");
        assert!(matches!(
            err,
            Error::RangeTooLarge {
                span: 401,
                max: WHOLE_FILE_LIMIT_LINES
            }
        ));
        assert!(err.to_string().contains("max is 400"));
    }

    #[test]
    fn char_budget_stops_early_with_true_end() {
        let long = "x".repeat(1000);
        let body = (1..=80)
            .map(|i| format!("line {i} {long}"))
            .collect::<Vec<_>>()
            .join("\n");
        let dir = workspace_with(&[("a.txt", &body)]);
        let hit = read(dir.path(), "a.txt", None, 100, None, WHOLE_FILE_LIMIT_LINES).expect("read");
        assert!(hit.end < hit.total);
        assert_eq!(hit.text.len() as u64, hit.end - hit.start + 1);
    }

    #[test]
    fn long_line_truncated_not_refused() {
        let long = "a".repeat(MAX_LINE_CHARS + 100);
        let content = format!("short\n{long}\n");
        let dir = workspace_with(&[("a.txt", content.as_str())]);
        let hit = read(dir.path(), "a.txt", None, 100, None, WHOLE_FILE_LIMIT_LINES).expect("read");
        assert_eq!(hit.text.len(), 2);
        assert!(hit.text[1].ends_with(TRUNCATED_LINE_MARKER));
    }

    #[test]
    fn format_numbered_lines_aligns() {
        let lines = vec!["a".to_owned(), "b".to_owned()];
        let out = format_numbered_lines(41, &lines);
        assert_eq!(out, vec!["41|a", "42|b"]);
    }

    #[test]
    fn symlink_outside_root_refused_read() {
        let outer = tempfile::tempdir().expect("outer");
        std::fs::write(outer.path().join("secret.txt"), "nope\n").expect("write");
        let dir = tempfile::tempdir().expect("inner");
        symlink(outer.path().join("secret.txt"), dir.path().join("link.txt")).expect("symlink");
        let err = read(
            dir.path(),
            "link.txt",
            None,
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect_err("escape");
        assert!(matches!(err, Error::Escape(_)));
    }

    #[test]
    fn symlink_inside_root_allowed_read() {
        let dir = workspace_with(&[("real.txt", "ok\n")]);
        symlink(dir.path().join("real.txt"), dir.path().join("link.txt")).expect("symlink");
        let hit = read(
            dir.path(),
            "link.txt",
            None,
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("read");
        assert_eq!(hit.text, vec!["ok"]);
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
