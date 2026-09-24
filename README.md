# toolgate

Harness-side guardrails — the other half of the token story. one-grep
narrows *search*; toolgate narrows what agents may *read*, *edit*, and
*run*. One deterministic **policy engine** (`src/policy.rs`) plus thin
per-harness adapters; telemetry is shared so `observe` mode is the vanilla
measurement arm.

## Policy (`~/.config/toolgate/policy.toml`)

| Rule | Default | Effect (enforce) |
| --- | --- | --- |
| `read.max_lines` | 400 | Deny whole-file reads without offset/limit |
| `glob.require_path_scope` | true | Deny unscoped glob at repo root |
| `grep.require_path_scope` | false | Same for grep when enabled |
| `shell` | built-in deny/allow prefixes | Deny dangerous argv (see `gate::Rules`) |
| `clip.min_bytes` | 4096 | Head+tail clip + archive large shell/MCP post output |
| `mode` | enforce | `observe` logs only; never changes harness I/O |

Overrides: `TOOLGATE_POLICY=enforce|observe`, `TOOLGATE_TELEMETRY=0` disables JSONL.

## CLI

```bash
toolgate read <file> [--line N] [--radius R] [--start M --end K] [--root <dir>]
toolgate edit <file> --old <text> --new <text> [--all] [--root <dir>]
toolgate run <program> [args...] [--root <dir>] [--timeout S]
toolgate serve --stdio   # MCP: read, edit, run (annotated hints)
toolgate hook --harness cursor --event preToolUse   # stdio JSON hook
```

Legacy: `toolgate hook --cursor-read` → Cursor `preToolUse`.

## Adapter matrix

| Harness | Hook events | Deny / rewrite (pre) | Replace output (post) | Source |
| --- | --- | --- | --- | --- |
| Cursor | `preToolUse`, `postToolUse`, `afterShellExecution`, `afterMCPExecution` | preToolUse | postToolUse MCP only; shell post via `additional_context` | [Cursor hooks](https://cursor.com/docs/agent/hooks) |
| Muse | `PreToolUse`, `PostToolUse` | same as Cursor | same | Muse settings (Claude Code–compatible JSON) |
| OpenCode | `tool.execute.before/after` | before | after `output` field | OpenCode plugin API |
| Pi | `tool_call` / `tool_result` | cancel + message | `content` text replace | Pi `ExtensionAPI` events (`epr.ts`) |

Shims (spawn `toolgate hook`, zero policy): `adapters/opencode/toolgate-hook.mjs`, `adapters/pi/toolgate-hook.mjs`.

## Install

**Nix:**

```bash
nix build .#toolgate
./result/bin/toolgate --version
```

Home Manager: `imports = [ inputs.toolgate.homeManagerModules.toolgate ];` then `programs.toolgate.enable = true;`.

**MCP (OpenCode example):**

```json
{ "mcp": { "toolgate": {
  "type": "local",
  "command": ["toolgate", "serve", "--stdio"],
  "enabled": true } } }
```

## Gates (library)

- `read`: bounded windows; symlink-safe root walks.
- `edit`: exact-string replace + diff.
- `run`: timeout + head/tail clip; optional Jev gate on MCP `run`.
- `hook`: fail-open JSON hooks + `events.jsonl` telemetry (digests only).
