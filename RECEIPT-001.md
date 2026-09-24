# RECEIPT-001 — T-flakes (tree A: toolgate)

## Built

- `flake.nix`: overlay, `packages.toolgate` / `default`, `devShells.default`, `checks.toolgate` (package + `cargo test` in checkPhase), `formatter` (nixfmt), systems x86_64-linux / aarch64-linux / aarch64-darwin.
- `nix/package.nix`: `rustPlatform.buildRustPackage`, `cleanSourceWith` on `lib.cleanSource` (excludes `target`, `result`/`result-*`, `RECEIPT-*.md`, `PACKET*.md`), lockfile-driven, `doCheck = true` (skips `judge_round_trips_systemone_wire` in sandbox), `meta.mainProgram = "toolgate"`.
- `README.md`: Nix install path (`nix build` / `nix develop`).

## Native inputs (why)

| Input | Why |
|-------|-----|
| `cmake` | `aws-lc-sys` (via `reqwest` / rustls) CMake build |
| `pkg-config` | transitive native deps for the Rust link graph |
| `rustPlatform.bindgenHook` | bindgen for `aws-lc-sys` |

## Gates (packet)

```text
$ nix build .#toolgate -L
toolgate>     Finished `release` profile [optimized] target(s) in 49.77s
toolgate> buildPhase completed in 50 seconds
toolgate> stripping (with command strip and flags -S -p) in .../bin

$ ./result/bin/toolgate --version
toolgate 0.1.0

$ nix develop -c cargo test -q
running 19 tests
...................
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.01s
```

## Review round 1

Grok NO-GO: `checks` was file-exists only; package had `doCheck = false`. Wired `checks.<system>.toolgate` to the derivation, enabled `doCheck`, `cleanSourceWith` on `lib.cleanSource` (drop `result`/`result-*`/`target` from src), skip one loopback-HTTP lib test under the Nix sandbox.

```text
$ nix build .#toolgate -L
toolgate> Finished cargoCheckHook
toolgate> stripping (with command strip and flags -S -p) in /nix/store/...-toolgate-0.1.0/bin

$ nix flake check -L
checking derivation checks.x86_64-linux.toolgate...
derivation evaluated to /nix/store/...-toolgate-0.1.0.drv
running 0 flake checks...
all checks passed!

$ nix log $(nix path-info .#checks.x86_64-linux.toolgate --derivation) | rg 'test result'
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 1.01s
```
