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
- Escapes, binaries, and generated files (>8k-char lines) are refused.

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
