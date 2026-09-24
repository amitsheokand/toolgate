# RECEIPT-001 — T-tg-run

## Shape

1. **Head + tail clip** — `run` streams stdout/stderr through a head buffer plus tail ring; output caps default to 8 KiB/stream (2 KiB head, 6 KiB tail) with `... [N bytes elided] ...`.
2. **Timeout keeps output** — timeouts return `RunHit { timed_out: true, code: -1, .. }` with clipped streams; `Error::Timeout` removed.
3. **Kill process group** — Unix spawn uses `process_group(0)`; timeout uses `killpg` + test for no `sleep 99999` grandchildren.
4. **No blocking in async** — MCP `read`/`edit`/`run` use `tokio::task::spawn_blocking`.
5. **Server gate policy** — `gate` removed from `RunParams`; `serve --gate off|jev` + `TOOLGATE_GATE`; `Rules` allow/deny prefixes, Jev only for unmatched argv (JSON array), reworded Noul, in-process cache.
6. **Tests without env races** — `Gate::new(api_key, base_url, model)`; `from_env` wrapper; gate tests do not set/remove env.
7. **Library features** — `default = ["server", "jev"]`; `--no-default-features` builds core lib + gate policy types only.

## New dependency

- **`libc`** (Unix target): `killpg` for process-group teardown on timeout.

## Gate output (tail)

```
running 36 tests
....................................
test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.34s

running 29 tests
.............................
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.34s
```

(`cargo fmt --check` clean.)

## Review round 1 (Grok NO-GO on f5ec3a4)

| # | Finding | Fix |
|---|---------|-----|
| 1 | `char_boundary_at_or_before` dropped last in-budget byte | Return `bytes.len()` when `pos >= len`; walk back only over UTF-8 continuation bytes; tests for ASCII identity and multibyte tail |
| 2 | Tail decoded with `from_utf8_lossy` mid-char; elided ignored boundary trim | Snap tail start/end to char boundaries; `elided = total - head_kept - tail_kept`; multibyte straddle test |
| 3 | Normal exit joined readers without killing the group | `kill_process_group_pgid` before reader joins on success and timeout; ignore `ESRCH`; test `bash -c 'sleep … & echo hi'` |
| 4 | Allow/deny was literal argv prefix match | Basename program; rm `-rf`/`-fr`/long flags; allowlist guards for cargo/git/rg; shells/find/env → Jev; bypass tests |
| 5 | Verdict cache keyed by argv only | `GateCacheKey { root, argv }` in `enforce` and MCP cache |
| 6 | `tokio` always required | `tokio` optional; enabled only by `server` and `jev` features |

### `cargo tree --no-default-features -e normal` (top level)

```
toolgate v0.1.0
├── libc v0.2.189
├── serde v1.0.229
├── serde_json v1.0.151
└── thiserror v2.0.21
```

## Review round 2 (Grok NO-GO)

| # | Finding | Fix |
|---|---------|-----|
| 1 | Head/tail clip left incomplete UTF-8 at buffer ends (`char_boundary_at_or_before` no-op when `pos >= len`) | Trim continuation bytes and shrink until `from_utf8` succeeds; test 2/3/4-byte scalars with exact elided byte arithmetic and no U+FFFD |
| 2 | `rules_verdict` ignored `Rules`; git deny looked at argv[1] only | Apply custom deny/allow prefixes (cargo/git/rg allow still uses guarded `allow_verdict`); skip git global options before push/reset deny; tests for `git -C … push --force`, `--no-pager push -f`, custom lists |

## Review round 3 (Grok NO-GO)

| # | Finding | Fix |
|---|---------|-----|
| 1 | Tail clip could panic when the tail window started on a UTF-8 continuation byte | Replaced boundary helpers with total `snap_start` / `snap_end`; property test over caps 0..=64 and scalar-mix splits (no panic, no U+FFFD, byte balance) |
| 2 | Allow could bypass deny; git globals incomplete | Fixed pipeline: `hard_deny` (built-ins + `Rules.deny`, git deny skips unknown globals) then `hard_allow`; unknown git globals fail closed on allow; expanded `git push` force detection; table-driven deny tests and `Rules.allow=["git"]` still blocks `git push -f` |

## Review round 4 (Grok NO-GO)

| # | Finding | Fix |
|---|---------|-----|
| 1 | Tail clip dropped the whole tail when the window started on a UTF-8 continuation | `snap_end` on `tail_window[tail_start..]`; property test checks head/tail byte slices match input |
| 2 | Closed `AllowSchema` replaces prefix auto-allow; `rules_verdict` takes `root` for path flags | Linear argv parser; deny on normalized git argv; table + fuzz tests |
| 3 | `-pp` short cluster advanced past `push`; bare `-` looped | One argv per cluster step; bare `-` is unknown and advances |
| 4 | `Rules.deny` on raw argv; unsafe git/cargo flags allowed | Deny uses normalized git tokens; block `--output`/`--textconv`/`--ext-diff`; cargo paths outside root fail closed |
| 5 | `rg --pre` handling | `--pre`/`--pre-glob` not in schema (Jev); safe `rg` still Allow |

## Review round 5 (Grok NO-GO)

| # | Finding | Fix |
|---|---------|-----|
| 1 | Relative and lexical-escape cargo paths skipped root checks | Shared [`read::inside_root`](src/read.rs): lexical `..` rejection, then canonical longest-existing ancestor under canonical root; all schema path flags and `ls` positionals use it |
| 2 | String-prefix fallback when `canonicalize` failed | Removed; no prefix checks |
| 3 | Git `-c` / `--config` / `--config-env` / `--exec-path` on allow path | Dropped from globals; `-c` routes unknown; cargo `--config`/`-Z`/`--pre`/`program` flags fail closed |
| 4 | `Rules.allow` for `ls` etc. bypassed schema | Every allow match requires `schema_allows` or bare program-only prefix (`argv.len() == prefix.len()`) |

## Review round 6 (Grok NO-GO)

| # | Finding | Fix |
|---|---------|-----|
| 1 | `lexical_under_root` applied `..` before symlink resolution; `link/../out` looked like `<root>/out` | `inside_root` walks components from canonical root, canonicalizing each existing prefix; non-existent tail is lexical without `..`; tests for `link/../out`, `link/../../x`, `a/../link/x` |
| 2 | `Rules.allow` exact-prefix match returned Allow when `schema_allows` was false | Allow only from `schema_allows`; programs without schema allow only `argv == [program]` via single-token `rules.allow`; tests for `cargo build --config …` and `git diff --textconv` prefixes |

## Gate output (tail)

```
running 53 tests
.....................................................
test result: ok. 53 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.34s

running 46 tests
..............................................
test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.34s
```

(`cargo fmt --check` clean.)
