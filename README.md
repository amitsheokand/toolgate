# toolgate

Harness-side guardrails — the other half of the token story. one-grep
narrows *search*; toolgate narrows what agents may *read*, *edit*, and
*run*. Separate repo and binary on purpose: one-grep stays read-only
search, never a gatekeeper.

## Now: capped reads

`read` returns bounded windows, never unbounded whole-file dumps:

```bash
toolgate read <file> [--line N] [--radius R] [--start M --end K] [--root <dir>]
toolgate edit <file> --old <text> --new <text> [--all] [--root <dir>]  # prints its diff
toolgate run <program> [args...] [--root <dir>] [--timeout S]          # argv-direct
toolgate serve  # MCP stdio server (`read`, `edit`, `run`)
toolgate hook cursor-read  # Cursor preToolUse Read cap (stdio JSON)

## Install

```bash
cargo install --git https://github.com/amitsheokand/toolgate
toolgate install  # not yet: register manually for now
```

opencode (`~/.config/opencode/opencode.json`, preserves peers):

```json
{ "mcp": { "toolgate": {
  "type": "local",
  "command": ["/home/amitsheokand/.local/bin/toolgate", "serve", "--stdio"],
  "enabled": true } } }
```

Restart the harness after (re)builds: the running server keeps serving
its loaded image.
```

- `--line N`: ~200-line window around N (default radius 100), clamped.
- No `--line`: files ≤ 400 lines read whole; larger files require
  `--start/--end` (or `--line`) — the error names the count and flags.
- Escapes (including symlink breakout), binaries (NUL), and overlong
  lines (truncated in read output with a marker) are handled as above.
- Read output prefixes each line with `N|` (1-based, right-aligned).

### Cursor `preToolUse` (Read)

Register in `~/.cursor/hooks.json` (merge with your existing hooks):

```json
{
  "hooks": {
    "preToolUse": [
      {
        "matcher": "Read",
        "command": "/home/amitsheokand/.local/bin/toolgate hook cursor-read"
      }
    ]
  }
}
```

Whole-file reads of files over 400 lines are **denied** with an
`agent_message` naming the line count (offset/limit reads pass through).
Set `TOOLGATE_READ_HOOK=0` to disable. The hook always exits 0 and prints
exactly one JSON object; I/O or parse failures emit `{"permission":"allow"}`.

## Gates

- `read`: windows by default, ranges on demand (above).
- `edit`: exact-string replacement returning its capped diff.
- `run`: timeout + output budget, argv-direct. `--gate` / `gate: true`
  asks the Jev safety Noul first (`TYPESAFE_API_KEY` required):
  allow runs, ask-band and block refuse with the score attached, and a
  missing key refuses too (fail closed). Thresholds live in one
  `gate::Policy` (`block_at` 0.65, `ask_at` 0.35), tested with canned
  numbers — tune from logs, not from the hook.

## Joining the measurement

one-grep's serving log (`~/.one-grep/serving.log`) records cited paths
per search; pi's `one-grep-hits` correlator logs reads. Join by
timestamp+path for "hits → did they still read the file?".
