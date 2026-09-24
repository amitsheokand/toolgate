# RECEIPT-001 — T-tg-policy

Commit: `c773cae` on `T-tg-policy`.

## Adapter matrix (sources)

| Harness | Events wired | Deny / rewrite (pre) | Output replace (post) | Citation |
| --- | --- | --- | --- | --- |
| Cursor | `preToolUse`, `postToolUse`, `afterShellExecution`, `afterMCPExecution` | `permission` + `updated_input` on pre | `updated_mcp_tool_output` on postToolUse (MCP); shell/MCP clip via `additional_context` | [Cursor Agent hooks](https://cursor.com/docs/agent/hooks) (`preToolUse` / `postToolUse` output tables) |
| Muse | `PreToolUse`, `PostToolUse` | Same JSON as Cursor pre/post | Same as Cursor | Muse `settings.json` hook groups (Claude Code–compatible); impl: `src/adapters/muse.rs` |
| OpenCode | `tool.execute.before` / `after` (shim sets `TOOLGATE_EVENT`) | `allow` / `message` | `output` string | `src/adapters/opencode.rs`, `adapters/opencode/toolgate-hook.mjs` |
| Pi | `tool_call` / `tool_result` (shim) | `cancel` + `message` | `content` text blocks | Pi `ExtensionAPI` / `tool_result` return shape; `src/adapters/pi.rs`, `adapters/pi/toolgate-hook.mjs` |

## Scripts to retire after parity

| Legacy | Replaced by |
| --- | --- |
| `harness-clip.py` (Cursor/Muse merged clip) | `policy` clip rule + `run::clip_utf8` + hook post handlers |
| `advait-clip.ts` / `epr.ts` clip path (Pi) | `toolgate hook --harness pi` + optional EPR non-clip logic |
| one-grep `deny-full-read.py` | `read.whole_file_cap` + Cursor/Muse `preToolUse` |

## Gate tails (2026-09-24)

```
$ nix develop -c cargo test -q
running 83 tests
...................................................................................
test result: ok. 83 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.34s
running 6 tests
......
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ nix develop -c cargo test -q --no-default-features
running 75 tests
...........................................................................
test result: ok. 75 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.34s
running 6 tests
......
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ nix develop -c cargo fmt --check
(exit 0)

$ nix build .#toolgate
this derivation will be built:
  /nix/store/8iq6mz895zrs2g45v6xmbapfgc58j6yz-toolgate-0.1.0.drv
building '/nix/store/8iq6mz895zrs2g45v6xmbapfgc58j6yz-toolgate-0.1.0.drv'...
(exit 0)
```

## Reviewer

Required: **grok-4.7-high** (per PACKET).
