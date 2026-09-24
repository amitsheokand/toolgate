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
    src = lib.cleanSource ../.;
    filter =
      path: _type:
      let
        name = baseNameOf path;
      in
      name != "target"
      && name != "result"
      && !(lib.hasPrefix "result-" name)
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

  doCheck = true;

  # Nix check sandbox has no network (loopback HTTP for mock TypeSafe API).
  cargoTestFlags = [
    "--"
    "--skip"
    "judge_round_trips_systemone_wire"
  ];

  meta = with lib; {
    description = "Harness-side guardrails: capped file reads and related gates";
    license = licenses.asl20;
    mainProgram = "toolgate";
    platforms = platforms.unix;
  };
}
