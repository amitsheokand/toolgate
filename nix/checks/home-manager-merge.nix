# Pure test: merge-hooks replaces toolgate entries on upgrade; peers preserved (Cursor + Muse shapes).
{ pkgs }:
let
  lib = pkgs.lib;
  jqBin = lib.getExe pkgs.jq;
  toolgateBin = lib.getExe pkgs.toolgate;
  substituteHook = src:
    pkgs.replaceVars src {
      TOOLGATE_BIN = toolgateBin;
    };
  opencodeHook = substituteHook ../../adapters/opencode/toolgate-hook.mjs;
  piHook = substituteHook ../../adapters/pi/toolgate-hook.ts;
  mergeHooks = import ../home-manager/merge-hooks.nix {
    inherit pkgs;
    toolgateBasename = "toolgate";
  };
  storeA = "/tmp/toolgate-fixture-store-a/bin/toolgate";
  storeB = "/tmp/toolgate-fixture-store-b/bin/toolgate";
  peerCursor = {
    matcher = "Other";
    command = "/usr/bin/my-peer-hook --stay";
  };
  peerCursorToolgateSubstring = {
    matcher = "Other";
    command = "/usr/bin/something-toolgate hook --harness cursor --event not-a-real-tg-hook";
  };
  toolgateCursor = harness: store: event: {
    matcher = "Read|Shell";
    command = "${store} hook --harness ${harness} --event ${event}";
  };
  fixtureCursor = pkgs.writeText "cursor-hooks-fixture.json" (builtins.toJSON {
    version = 1;
    hooks = {
      preToolUse = [
        peerCursor
        peerCursorToolgateSubstring
        (toolgateCursor "cursor" storeA "preToolUse")
      ];
    };
  });
  patchCursorA = pkgs.writeText "cursor-patch-a.json" (builtins.toJSON {
    version = 1;
    hooks = {
      preToolUse = [
        (toolgateCursor "cursor" storeA "preToolUse")
      ];
    };
  });
  patchCursorB = pkgs.writeText "cursor-patch-b.json" (builtins.toJSON {
    version = 1;
    hooks = {
      preToolUse = [
        (toolgateCursor "cursor" storeB "preToolUse")
      ];
    };
  });
  patchCursorEmpty = pkgs.writeText "cursor-patch-empty.json" (builtins.toJSON {
    version = 1;
    hooks = { };
  });
  peerMuse = {
    matcher = "Other";
    hooks = [
      {
        type = "command";
        command = "/usr/bin/muse-peer --stay";
      }
    ];
  };
  toolgateMuse = store: event: {
    matcher = "Read|Shell";
    hooks = [
      {
        type = "command";
        command = "${store} hook --harness muse --event ${event}";
      }
    ];
  };
  fixtureMuse = pkgs.writeText "muse-settings-fixture.json" (builtins.toJSON {
    hooks = {
      PreToolUse = [
        peerMuse
        (toolgateMuse storeA "PreToolUse")
      ];
    };
  });
  patchMuseA = pkgs.writeText "muse-patch-a.json" (builtins.toJSON {
    hooks = {
      PreToolUse = [
        (toolgateMuse storeA "PreToolUse")
      ];
    };
  });
  patchMuseB = pkgs.writeText "muse-patch-b.json" (builtins.toJSON {
    hooks = {
      PreToolUse = [
        (toolgateMuse storeB "PreToolUse")
      ];
    };
  });
  jq = jqBin;
in
pkgs.stdenv.mkDerivation {
  name = "toolgate-home-manager-merge-test";
  nativeBuildInputs = [ pkgs.jq ];
  dontUnpack = true;
  phases = [ "installPhase" ];
  installPhase = ''
    work=$(mktemp -d)
    merge=${mergeHooks}
    jq=${jq}

    for shim in ${opencodeHook} ${piHook}; do
      if grep -q '@TOOLGATE_BIN@' "$shim"; then
        echo "placeholder not substituted in $shim" >&2
        exit 1
      fi
      if ! grep -q '${toolgateBin}' "$shim"; then
        echo "expected store binary in $shim" >&2
        exit 1
      fi
    done

    cursor_out="$work/cursor-hooks.json"
    cp ${fixtureCursor} "$cursor_out"
    $merge "$cursor_out" ${patchCursorA}
    $merge "$cursor_out" ${patchCursorB}
    expected="${storeB} hook --harness cursor --event preToolUse"
    peer_count=$($jq '[.hooks.preToolUse[] | select(.command == "/usr/bin/my-peer-hook --stay")] | length' "$cursor_out")
    substring_peer=$($jq '[.hooks.preToolUse[] | select(.command == "/usr/bin/something-toolgate hook --harness cursor --event not-a-real-tg-hook")] | length' "$cursor_out")
    tg_count=$($jq '[.hooks.preToolUse[] | select(.command | test("hook --harness cursor --event preToolUse"))] | length' "$cursor_out")
    tg_cmd=$($jq -r '.hooks.preToolUse[] | select(.command | test("hook --harness cursor --event preToolUse")) | .command' "$cursor_out")
    test "$peer_count" = "1"
    test "$substring_peer" = "1"
    test "$tg_count" = "1"
    test "$tg_cmd" = "$expected"

    stale_out="$work/cursor-stale.json"
    cp ${fixtureCursor} "$stale_out"
    $merge "$stale_out" ${patchCursorEmpty}
    stale_tg=$($jq '[.hooks.preToolUse[] | select(.command | test("^/tmp/toolgate-fixture-store"))] | length' "$stale_out")
    stale_peers=$($jq '.hooks.preToolUse | length' "$stale_out")
    test "$stale_tg" = "0"
    test "$stale_peers" = "2"

    empty_out="$work/cursor-empty.json"
    : > "$empty_out"
    $merge "$empty_out" ${patchCursorB}
    empty_tg=$($jq '[.hooks.preToolUse[] | select(.command | test("hook --harness cursor"))] | length' "$empty_out")
    test "$empty_tg" = "1"

    missing_out="$work/nested/missing-hooks.json"
    rm -f "$missing_out"
    $merge "$missing_out" ${patchCursorB}
    test -f "$missing_out"
    missing_tg=$($jq '[.hooks.preToolUse[] | select(.command | test("hook --harness cursor"))] | length' "$missing_out")
    test "$missing_tg" = "1"

    invalid_out="$work/cursor-invalid.json"
    printf '%s' '{ not valid json' > "$invalid_out"
    invalid_before=$(cat "$invalid_out")
    invalid_stderr="$work/invalid.stderr"
    set +e
    $merge "$invalid_out" ${patchCursorA} 2>"$invalid_stderr"
    invalid_status=$?
    set -e
    test "$invalid_status" = "0"
    test "$(cat "$invalid_out")" = "$invalid_before"
    grep -q warning "$invalid_stderr"

    muse_out="$work/muse-settings.json"
    cp ${fixtureMuse} "$muse_out"
    $merge "$muse_out" ${patchMuseA}
    $merge "$muse_out" ${patchMuseB}
    expected_muse="${storeB} hook --harness muse --event PreToolUse"
    muse_peer=$($jq '[.hooks.PreToolUse[] | select(.hooks[0].command == "/usr/bin/muse-peer --stay")] | length' "$muse_out")
    muse_tg=$($jq '[.hooks.PreToolUse[] | select(.hooks[0].command | test("hook --harness muse --event PreToolUse"))] | length' "$muse_out")
    muse_cmd=$($jq -r '.hooks.PreToolUse[] | .hooks[0].command | select(test("hook --harness muse"))' "$muse_out")
    test "$muse_peer" = "1"
    test "$muse_tg" = "1"
    test "$muse_cmd" = "$expected_muse"

    touch "$out"
  '';
}
