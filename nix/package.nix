# Build toolgate with nixpkgs rustPlatform.
# OSS-safe: no private hostnames, product names, or user paths.
{
  lib,
  rustPlatform,
  cmake,
  pkg-config,
  cacert,
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

  # reqwest's TLS backend fails to build a client without a CA bundle,
  # even for the loopback mock TypeSafe API used by the Jev tests.
  nativeCheckInputs = [ cacert ];
  preCheck = ''
    export SSL_CERT_FILE=${cacert}/etc/ssl/certs/ca-bundle.crt
  '';

  meta = with lib; {
    description = "Harness-side guardrails: capped file reads and related gates";
    license = licenses.asl20;
    mainProgram = "toolgate";
    platforms = platforms.unix;
  };
}
