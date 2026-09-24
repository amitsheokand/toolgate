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

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

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

/// Count lines in a UTF-8 text file at `path` (binary or unreadable → `None`).
#[must_use]
pub fn count_file_lines(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    if text.is_empty() {
        return Some(0);
    }
    Some(text.lines().count() as u64)
}

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

/// Resolve `raw` under `root`, rejecting lexical and symlink escapes.
///
/// Returns the **canonical** path used for all later I/O.
pub(crate) fn resolve_path(root: &Path, raw: &str) -> Result<PathBuf, Error> {
    inside_root(root, raw).ok_or_else(|| Error::Escape(raw.to_owned()))
}

/// Metadata for an existing file at a canonical path (no symlink follow on open).
pub(crate) fn metadata_nofollow(path: &Path) -> Result<std::fs::Metadata, Error> {
    let file = open_nofollow(path, false)?;
    file.metadata().map_err(Error::from)
}

/// Open `path` without following symlinks; verify fd identity matches path metadata.
#[cfg(unix)]
fn open_nofollow(path: &Path, write: bool) -> Result<File, Error> {
    let meta = fs::metadata(path).map_err(Error::from)?;
    let mut opts = OpenOptions::new();
    opts.custom_flags(libc::O_NOFOLLOW);
    if write {
        opts.write(true);
    } else {
        opts.read(true);
    }
    let file = opts.open(path).map_err(Error::from)?;
    let fd = file.as_raw_fd();
    let fd_path = PathBuf::from(format!("/proc/self/fd/{fd}"));
    let fd_meta = fs::metadata(&fd_path).map_err(Error::from)?;
    if meta.dev() != fd_meta.dev() || meta.ino() != fd_meta.ino() {
        return Err(Error::Escape(format!(
            "path identity changed during open: {}",
            path.display()
        )));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_nofollow(path: &Path, write: bool) -> Result<File, Error> {
    let mut opts = OpenOptions::new();
    if write {
        opts.write(true);
    } else {
        opts.read(true);
    }
    opts.open(path).map_err(Error::from)
}

/// Read file bytes at canonical `path` without following symlinks.
pub(crate) fn read_bytes_nofollow(path: &Path) -> Result<Vec<u8>, Error> {
    let mut file = open_nofollow(path, false)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(Error::from)?;
    Ok(bytes)
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
    let bytes = read_bytes_nofollow(&path)?;
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
        return line.to_owned();
    }
    let mut end = MAX_LINE_CHARS;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    let mut s = line[..end].to_owned();
    s.push_str(TRUNCATED_LINE_MARKER);
    s
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
    fn range_exactly_400_allowed_401_refused() {
        let dir = workspace_with(&[("a.txt", &numbered(500))]);
        let hit = read(
            dir.path(),
            "a.txt",
            None,
            100,
            Some((1, 400)),
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("400 ok");
        assert_eq!((hit.start, hit.end), (1, 400));
        let err = read(
            dir.path(),
            "a.txt",
            None,
            100,
            Some((1, 401)),
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect_err("401");
        assert!(matches!(
            err,
            Error::RangeTooLarge {
                span: 401,
                max: WHOLE_FILE_LIMIT_LINES
            }
        ));
    }

    #[test]
    fn empty_file_and_no_trailing_newline() {
        let dir = workspace_with(&[("empty.txt", ""), ("plain.txt", "solo")]);
        let hit = read(
            dir.path(),
            "empty.txt",
            None,
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("empty");
        assert_eq!((hit.start, hit.end, hit.total), (1, 0, 0));
        assert!(hit.text.is_empty());
        let hit = read(
            dir.path(),
            "plain.txt",
            None,
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("plain");
        assert_eq!(hit.total, 1);
        assert_eq!(hit.text, vec!["solo"]);
    }

    #[test]
    fn multibyte_long_line_truncated_safely() {
        let long = "€".repeat(MAX_LINE_CHARS);
        let content = format!("short\n{long}\n");
        let dir = workspace_with(&[("a.txt", content.as_str())]);
        let hit = read(dir.path(), "a.txt", None, 100, None, WHOLE_FILE_LIMIT_LINES).expect("read");
        assert_eq!(hit.text.len(), 2);
        assert!(hit.text[1].ends_with(TRUNCATED_LINE_MARKER));
        assert!(std::str::from_utf8(hit.text[1].as_bytes()).is_ok());
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
    fn resolve_refuses_dir_symlink_outside_for_new_file() {
        let outer = tempfile::tempdir().expect("outer");
        let dir = tempfile::tempdir().expect("inner");
        std::fs::create_dir_all(dir.path().join("sub")).expect("mkdir");
        symlink(outer.path(), dir.path().join("sub/out")).expect("symlink dir");
        let err = resolve_path(dir.path(), "sub/out/new.txt").expect_err("escape");
        assert!(matches!(err, Error::Escape(_)));
    }

    #[test]
    fn resolve_refuses_read_through_dir_symlink_to_sibling_outside() {
        let parent = tempfile::tempdir().expect("parent");
        let ws = parent.path().join("ws");
        let outside = parent.path().join("outside");
        fs::create_dir_all(&ws).expect("ws");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("secret.txt"), "nope\n").expect("write");
        symlink("../outside", ws.join("link")).expect("symlink");
        let err = read(
            ws.as_path(),
            "link/secret.txt",
            None,
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect_err("escape");
        assert!(matches!(err, Error::Escape(_)));
    }

    #[test]
    fn root_symlink_workspace_still_reads() {
        let real = tempfile::tempdir().expect("real");
        std::fs::write(real.path().join("a.txt"), "hi\n").expect("write");
        let link_root = tempfile::tempdir().expect("link parent");
        symlink(real.path(), link_root.path().join("ws")).expect("root symlink");
        let hit = read(
            link_root.path().join("ws").as_path(),
            "a.txt",
            None,
            100,
            None,
            WHOLE_FILE_LIMIT_LINES,
        )
        .expect("read through symlink root");
        assert_eq!(hit.text, vec!["hi"]);
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
    }
}
