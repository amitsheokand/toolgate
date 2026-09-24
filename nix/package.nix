# Build toolgate with nixpkgs rustPlatform.
# OSS-safe: no private hostnames, product names, or user paths.
{
  lib,
  rustPlatform,
  cmake,
  pkg-config,
}:

rustPlatform.buildRustPackage {
  pname = "toolgate";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: _type:
      let
        name = baseNameOf path;
      in
      name != "target"
      && name != ".git"
      && name != ".DS_Store"
      && !(lib.hasSuffix ".md" name && lib.hasPrefix "RECEIPT-" name)
      && !(lib.hasSuffix ".md" name && lib.hasPrefix "PACKET" name);
  };

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    cmake
    pkg-config
    rustPlatform.bindgenHook
  ];

  # Package check deferred; use `cargo test` / host validation instead.
  doCheck = false;

  meta = with lib; {
    description = "Harness-side guardrails: capped file reads and related gates";
    license = licenses.asl20;
    mainProgram = "toolgate";
    platforms = platforms.unix;
  };
}
