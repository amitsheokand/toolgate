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
running 40 tests
........................................
test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

(`cargo fmt --check` passed; no new dependencies.)
