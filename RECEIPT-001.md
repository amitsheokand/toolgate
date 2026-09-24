# RECEIPT-001 — T-tg-policy

Commit: `8fd115c` on `T-tg-policy`.

## Adapter matrix (sources)

| Harness | Events wired | Deny / rewrite (pre) | Output replace (post) | Citation |
| --- | --- | --- | --- | --- |
| Cursor | `preToolUse`, `postToolUse`, `afterShellExecution`, `afterMCPExecution` | `permission` + `updated_input` on pre | `updated_mcp_tool_output` on `postToolUse` only; after-hooks telemetry-only (no model I/O change) | [Cursor Agent hooks](https://cursor.com/docs/agent/hooks) |
| Muse | `PreToolUse`, `PostToolUse` | `hookSpecificOutput` + `hookEventName: PreToolUse` | PostToolUse `hookSpecificOutput.updatedToolOutput` + `hookEventName: PostToolUse` | Claude Code–compatible Muse settings (`hooks[].hooks[{type,command}]`) |
| OpenCode | `tool.execute.before` / `after` via plugin export | shim throws on `{deny}`; `{args}` merge on before | `output.output` on after | OpenCode plugin handlers; `adapters/opencode/toolgate-hook.mjs` |
| Pi | `tool_call` / `tool_result` extension handlers | `{block, reason}` on `tool_call`; `{input}` merge for rewrite | `{content: [{type,text}]}` on `tool_result` | [Pi extensions](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md); `epr.ts` `commandOf` / `input` |

## Scripts to retire after parity

| Legacy | Replaced by |
| --- | --- |
| `harness-clip.py` (Cursor/Muse merged clip) | `policy` clip rule + `run::clip_utf8` + harness post handlers (MCP replace where supported) |
| `advait-clip.ts` / `epr.ts` clip path (Pi) | `toolgate hook --harness pi` + optional EPR non-clip logic |
| one-grep `deny-full-read.py` | `read.whole_file_cap` + harness pre hooks |

## Gate tails (2026-09-25, Round 3)

```
$ nix develop -c cargo test -q
running 95 tests
....................................................................................... 87/95
........
test result: ok. 95 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.94s
running 11 tests
...........
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

$ nix develop -c cargo test -q --no-default-features
running 87 tests
....................................................................................... 87/87
test result: ok. 87 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.44s
running 11 tests
...........
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

$ nix develop -c cargo fmt --check
(exit 0)

$ nix build .#toolgate
this derivation will be built:
  /nix/store/04wz1jxp6lpa53i8vywfcc6373bxi7f4-toolgate-0.1.0.drv
building '/nix/store/04wz1jxp6lpa53i8vywfcc6373bxi7f4-toolgate-0.1.0.drv'...
(exit 0)
```

## Review round 2 (grok re-review NO-GO → fixed)

- Cursor: classify from `tool_name` when `hook_event_name` is `preToolUse`/`postToolUse`; MCP post clips use `mcp_server_name` + tool name (not `MCP:` prefix only).
- Muse: `hookEventName` on all PreToolUse replies; PostToolUse clips via `hookSpecificOutput.updatedToolOutput`.
- Pi: `commandOf` parity with `epr.ts` (`then_run` object on `input` / string on `details`).
- Hook capture: `--record DIR` / `TOOLGATE_RECORD_DIR` → redacted JSONL per harness event (README).
- Fixtures: Cursor shell deny + MCP post with common schema; Muse `big_shell_output` matrix case.

## Review round 1 (grok NO-GO → fixed)

- Muse PreToolUse: `hookSpecificOutput` shape; Bash → Shell; HM nested `hooks` + merge-by-command.
- Cursor: classify MCP after-hooks from `hook_event_name`; shell/MCP after-hooks do not emit `additional_context`; `postToolUse` without bogus `MCP:*` matcher.
- OpenCode/Pi: real plugin/extension shims (no stdin-only scripts); opencode `{deny,args,output}`; Pi `input` parse + `{block,reason}` / `tool_result` content.
- Policy: no archive writes in `observe` mode; shell post-clip only on OpenCode/Pi.
- Goldens: full packet matrix per harness under `tests/fixtures/<harness>/` (+ Cursor big shell/MCP cases).

## Reviewer

Required: **grok-4.7-high** (per PACKET).
