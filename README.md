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
```

- `--line N`: ~200-line window around N (default radius 100), clamped.
- No `--line`: files ≤ 400 lines read whole; larger files require
  `--start/--end` (or `--line`) — the error names the count and flags.
- Escapes, binaries, and generated files (>8k-char lines) are refused.

## Queued (in order)

1. **Edit diffs** (advice #4): return a short diff with each edit so agents
   stop re-reading files to check their own writes.
2. **Shell gates** (advice #5): bounded/long-running command policy
   (`run_gates` equivalent) — the biggest remaining re-read source after
   reads.

## Joining the measurement

one-grep's serving log (`~/.one-grep/serving.log`) records cited paths
per search; pi's `one-grep-hits` correlator logs reads. Join by
timestamp+path for "hits → did they still read the file?".
