# RECEIPT-001 — T-tg-policy

Commit: `c78920f` on `T-tg-policy`.

## Adapter matrix (sources)

| Harness | Events wired | Deny / rewrite (pre) | Output replace (post) | Citation |
| --- | --- | --- | --- | --- |
| Cursor | `preToolUse`, `postToolUse`, `afterShellExecution`, `afterMCPExecution` | `permission` + `updated_input` on pre | `updated_mcp_tool_output` on `postToolUse` only; after-hooks telemetry-only (no model I/O change) | [Cursor Agent hooks](https://cursor.com/docs/agent/hooks) |
| Muse | `PreToolUse`, `PostToolUse` | `hookSpecificOutput.permissionDecision` / `permissionDecisionReason` / `updatedInput` on PreToolUse | PostToolUse MCP clip (Cursor-shaped `updated_mcp_tool_output`) | Claude Code–compatible Muse settings (`hooks[].hooks[{type,command}]`) |
| OpenCode | `tool.execute.before` / `after` via plugin export | shim throws on `{deny}`; `{args}` merge on before | `output.output` on after | OpenCode plugin handlers; `adapters/opencode/toolgate-hook.mjs` |
| Pi | `tool_call` / `tool_result` extension handlers | `{block, reason}` on `tool_call`; `{input}` merge for rewrite | `{content: [{type,text}]}` on `tool_result` | [Pi extensions](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md); `epr.ts` `commandOf` / `input` |

## Scripts to retire after parity

| Legacy | Replaced by |
| --- | --- |
| `harness-clip.py` (Cursor/Muse merged clip) | `policy` clip rule + `run::clip_utf8` + harness post handlers (MCP replace where supported) |
| `advait-clip.ts` / `epr.ts` clip path (Pi) | `toolgate hook --harness pi` + optional EPR non-clip logic |
| one-grep `deny-full-read.py` | `read.whole_file_cap` + harness pre hooks |

## Gate tails (2026-09-24, Round 2)

```
$ nix develop -c cargo test -q
running 90 tests
..........................................................................................
test result: ok. 90 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.38s
running 11 tests
...........
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

$ nix develop -c cargo test -q --no-default-features
running 82 tests
..................................................................................
test result: ok. 82 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.40s
running 11 tests
...........
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

$ nix develop -c cargo fmt --check
(exit 0)

$ nix build .#toolgate
this derivation will be built:
  /nix/store/js9kkbhff9nnx1dvazn2zx8yza10n1zq-toolgate-0.1.0.drv
building '/nix/store/js9kkbhff9nnx1dvazn2zx8yza10n1zq-toolgate-0.1.0.drv'...
(exit 0)
```

## Review round 1 (grok NO-GO → fixed)

- Muse PreToolUse: `hookSpecificOutput` shape; Bash → Shell; HM nested `hooks` + merge-by-command.
- Cursor: classify MCP after-hooks from `hook_event_name`; shell/MCP after-hooks do not emit `additional_context`; `postToolUse` without bogus `MCP:*` matcher.
- OpenCode/Pi: real plugin/extension shims (no stdin-only scripts); opencode `{deny,args,output}`; Pi `input` parse + `{block,reason}` / `tool_result` content.
- Policy: no archive writes in `observe` mode; shell post-clip only on OpenCode/Pi.
- Goldens: full packet matrix per harness under `tests/fixtures/<harness>/` (+ Cursor big shell/MCP cases).

## Reviewer

Required: **grok-4.7-high** (per PACKET).
