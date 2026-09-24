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
running 27 tests
...........................
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.33s

running 20 tests
....................
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.33s
```

(`cargo fmt --check` clean.)
