# RECEIPT-001 — T-flakes (tree A: toolgate)

## Built

- `flake.nix`: overlay, `packages.toolgate` / `default`, `devShells.default`, cheap `checks.package-file`, `formatter` (nixfmt), systems x86_64-linux / aarch64-linux / aarch64-darwin.
- `nix/package.nix`: `rustPlatform.buildRustPackage`, `cleanSourceWith` (excludes `target`, `.git`, `RECEIPT-*.md`, `PACKET*.md`), lockfile-driven, `doCheck = false`, `meta.mainProgram = "toolgate"`.
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
