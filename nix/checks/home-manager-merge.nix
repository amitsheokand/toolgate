# Pure test: merge-hooks replaces toolgate entries on upgrade; peers preserved (Cursor + Muse shapes).
{ pkgs }:
let
  lib = pkgs.lib;
  jqBin = lib.getExe pkgs.jq;
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
  toolgateCursor = harness: store: event: {
    matcher = "Read|Shell";
    command = "${store} hook --harness ${harness} --event ${event}";
  };
  fixtureCursor = pkgs.writeText "cursor-hooks-fixture.json" (builtins.toJSON {
    version = 1;
    hooks = {
      preToolUse = [
        peerCursor
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
pkgs.runCommand "toolgate-home-manager-merge-test"
  {
    nativeBuildInputs = [ pkgs.jq ];
  }
  ''
    work=$(mktemp -d)
    merge=${mergeHooks}
    jq=${jq}

    cursor_out="$work/cursor-hooks.json"
    cp ${fixtureCursor} "$cursor_out"
    $merge "$cursor_out" ${patchCursorA}
    $merge "$cursor_out" ${patchCursorB}
    expected="${storeB} hook --harness cursor --event preToolUse"
    peer_count=$($jq '[.hooks.preToolUse[] | select(.command == "/usr/bin/my-peer-hook --stay")] | length' "$cursor_out")
    tg_count=$($jq '[.hooks.preToolUse[] | select(.command | test("hook --harness cursor --event preToolUse"))] | length' "$cursor_out")
    tg_cmd=$($jq -r '.hooks.preToolUse[] | select(.command | test("hook --harness cursor")) | .command' "$cursor_out")
    test "$peer_count" = "1"
    test "$tg_count" = "1"
    test "$tg_cmd" = "$expected"

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

    touch $out
  ''
