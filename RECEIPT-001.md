# RECEIPT-001 — T-tg-hm

## Gates

```bash
nix flake check -L && nix build .#toolgate
```

### `nix flake check -L` (tail)

```
checking derivation checks.x86_64-linux.home-manager-merge...
derivation evaluated to /nix/store/krhrlszrc750mgggikmzsyqv05nkdhwv-toolgate-home-manager-merge-test.drv
checking flake output 'formatter'...
checking derivation formatter.x86_64-linux...
derivation evaluated to /nix/store/ww8q66nz1q66mkking8k9wacy0w7wk9a-nixfmt-1.5.0.drv
checking flake output 'homeManagerModules'...
warning: unknown flake output 'homeManagerModules'
running 3 flake checks...
building '/nix/store/krhrlszrc750mgggikmzsyqv05nkdhwv-toolgate-home-manager-merge-test.drv'...
building '/nix/store/lzhrxg0ajwiacdyswm7bspx73yizix50-toolgate-0.1.0.drv'...
toolgate> test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
toolgate>
toolgate>    Doc-tests toolgate
toolgate>
toolgate> running 0 tests
toolgate>
toolgate> test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
toolgate>
toolgate> Finished cargoCheckHook
toolgate> Running phase: installPhase
toolgate> Executing cargoInstallHook
toolgate> Finished cargoInstallHook
toolgate> Running phase: fixupPhase
toolgate> shrinking RPATHs of ELF executables and libraries in /nix/store/nf7kh2x985b1kdyabm6r1yy5d331pg7r-toolgate-0.1.0
toolgate> shrinking /nix/store/nf7kh2x985b1kdyabm6r1yy5d331pg7r-toolgate-0.1.0/bin/toolgate
toolgate> checking for references to /build/ in /nix/store/nf7kh2x985b1kdyabm6r1yy5d331pg7r-toolgate-0.1.0...
toolgate> patching script interpreter paths in /nix/store/nf7kh2x985b1kdyabm6r1yy5d331pg7r-toolgate-0.1.0
toolgate> stripping (with command strip and flags -S -p) in /nix/store/nf7kh2x985b1kdyabm6r1yy5d331pg7r-toolgate-0.1.0/bin
all checks passed!
warning: The check omitted these incompatible systems: aarch64-darwin, aarch64-linux
Use '--all-systems' to check all.
```

### `nix build .#toolgate` (tail)

First run (same session as check) produced `/nix/store/nf7kh2x985b1kdyabm6r1yy5d331pg7r-toolgate-0.1.0` with exit 0. Subsequent build was a cache hit (no log lines).

```
warning: Git tree '/home/amitsheokand/work/worktrees/toolgate/T-tg-hm' is dirty
```

## Summary

- Home Manager merge keys toolgate hooks by `hook --harness … --event …`, drops stale toolgate rows, keeps peers.
- Pi shim: `adapters/pi/toolgate-hook.ts` → `~/.pi/agent/extensions/` with store-path binary via `replaceVars`.
- OpenCode shim: store-path binary via `replaceVars`.
- `programs.toolgate.harnesses` gates hook installs; `mode` only in `policy.toml`; removed `mcp.enable` / `TOOLGATE_POLICY` session var.
- Flake check `home-manager-merge` exercises Cursor + Muse merge across two synthetic store paths.

## Review round 1

Round 2 fixes from grok NO-GO: correct `replaceVars` call shape; merge treats empty/missing targets as `{}`, invalid JSON warns and exits 0; toolgate detection uses basename argv token; cursor/muse merge always runs (empty patch when harness disabled); flake check builds Pi/OpenCode shims (no `@TOOLGATE_BIN@`), empty/missing/invalid targets, substring peer, stale toolgate removal.

### `nix flake check -L` (tail)

```
building '/nix/store/a55rl01hg61fl0bfhqnp5nmaks94syy0-toolgate-home-manager-merge-test.drv'...
toolgate-home-manager-merge-test> Running phase: installPhase
all checks passed!
warning: The check omitted these incompatible systems: aarch64-darwin, aarch64-linux
Use '--all-systems' to check all.
```

### `nix build .#toolgate` (tail)

```
warning: Git tree '/home/amitsheokand/work/worktrees/toolgate/T-tg-hm' is dirty
```

(Cache hit after `nix flake check`; no rebuild log lines.)
