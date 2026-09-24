# RECEIPT-001 — T-tg-safety

## Shape (by item)

1. **Symlink escape:** `resolve_path` canonicalizes workspace root and target (parent when missing); refuses `Error::Escape` when canonical target leaves root. Tests for read and edit, in and out.
2. **Edit byte-preserving:** `load_file` strict UTF-8; replacement on full string; atomic temp+rename with permission copy; CRLF and non-UTF-8 tests.
3. **Diff context:** Up to 3 context lines each side; `@@` counts include context; `DIFF_CAP_LINES` unchanged.
4. **Read caps:** `RangeTooLarge` when explicit span > 400; `MAX_RADIUS` 200 clamp; `READ_CHAR_BUDGET` shrinks `end`; long lines truncated in output; NUL still binary.
5. **Line numbers:** `format_numbered_lines` used by CLI and MCP (`N|line`, width from last line in chunk).
6. **cursor-read hook:** `src/hook.rs` + `toolgate hook cursor-read`; decision tests; README `hooks.json` snippet; `TOOLGATE_READ_HOOK=0` fail-open.
7. **lib export:** `pub mod hook`.

## Hook response choice

**`permission: "allow"` + `updated_input.limit: 400`** (no `agent_message`).

Cursor docs (`preToolUse` output table) state `agent_message` is fed back **when the action is denied**. Deny would block the Read entirely, so the cap must be applied via `updated_input`. The packet’s explanatory text is not attached on allow; agents still get a 400-line read.

## Gate output (tail)

```
running 45 tests
.............................................
test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

(`cargo fmt --check` passed; no new dependencies.)

## Review round 1

| # | Finding | Fix |
|---|---------|-----|
| 1 | Lexical path reused after canonicalize | `resolve_path` returns canonical path; `read_bytes_nofollow` / `metadata_nofollow` open with `O_NOFOLLOW` and verify fd `(dev,ino)` vs path metadata; edit temp+rename in canonical parent. **Residual:** parent directory swap between `metadata` and `open` is not `openat2`/`RESOLVE_BENEATH`; TOCTOU on parent remains. Added unix `libc` dep (see `Cargo.toml`, out of parallel fence). |
| 2 | Byte slice on multibyte char | `truncate_line_display` walks back to `is_char_boundary` before cut. |
| 3 | Predictable temp path / symlink follow | `create_new` + `O_NOFOLLOW`, unique `.toolgate-{pid}-{seq}-{nanos}`; `remove_file` on any failure path. |
| 4 | Silent allow+cap | Large whole-file reads → `permission: "deny"` + `agent_message` with line count and offset/limit / one-grep guidance. |
| 5 | Hook error paths | `cursor_read_stdio_with` fail-open; `emit_output` fallback JSON; `main` prints allow if stdio returns `Err`. |
| 6 | Test gaps | Added cases: `@@` headers line 1 / last line no NL, symlink dir outside, sibling escape, symlink root, multibyte truncate, empty/no-NL file, range 400/401, BOM, overlapping `applied`, temp cleanup, planted temp symlink, hook deny + stdio tests. |
| 7 | Process env in tests | `decide_cursor_read(..., read_hook_env: Option<&str>)`; tests pass env explicitly. |

### Gate output (tail, round 1)

```
running 45 tests
.............................................
test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## Review round 2

| # | Finding | Fix |
|---|---------|-----|
| 1 | `metadata_nofollow` opened with `O_NOFOLLOW` then called `fs::metadata(path)` (follows symlinks) | Return `file.metadata()` from the nofollow fd. |
| 2 | Multibyte truncate test used 2-byte `é`; cut at `MAX_LINE_CHARS` bytes already on a char boundary | Repeat 3-byte `€` so the byte cut lands mid-scalar and `is_char_boundary` walk-back is covered. |

### Gate output (tail, round 2)

```
running 45 tests
.............................................
test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

(`cargo fmt --check` passed.)
