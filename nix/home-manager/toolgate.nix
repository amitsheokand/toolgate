# Home Manager module: `programs.toolgate`
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.toolgate;
  toolgateBin = lib.getExe cfg.package;
  policyFile = "${config.xdg.configHome}/toolgate/policy.toml";
  mergeHooks = pkgs.writeShellScript "toolgate-merge-hooks" ''
    set -euo pipefail
    target="$1"
    shift
    tmp=$(mktemp)
    if [ -f "$target" ]; then
      cp "$target" "$tmp"
    else
      echo '{}' > "$tmp"
    fi
    merge_jq='
      def command_key(e):
        if (e | type) != "object" then null
        elif e.command? then e.command
        elif e.hooks? and (e.hooks | length) > 0 then e.hooks[0].command
        else null end;
      def merge_lists($base; $patch):
        ($base // []) as $b | ($patch // []) as $p |
        ($p | map(command_key(.))) as $keys |
        ($b | map(select(command_key(.) as $k | $keys | index($k) == null))) + $p;
      def merge_hooks($base; $patch):
        ($base.hooks // {}) as $bh | ($patch.hooks // {}) as $ph |
        reduce (($bh | keys) + ($ph | keys) | unique | .[]) as $k
          ({}; . + { ($k): merge_lists($bh[$k]; $ph[$k]) }) as $merged |
        $base * $patch | .hooks = $merged;
      merge_hooks(.[0]; .[1])
    '
    for patch in "$@"; do
      ${pkgs.jq}/bin/jq -s "$merge_jq" "$tmp" "$patch" > "$tmp.new"
      mv "$tmp.new" "$tmp"
    done
    mkdir -p "$(dirname "$target")"
    mv "$tmp" "$target"
  '';
in
{
  options.programs.toolgate = {
    enable = lib.mkEnableOption "toolgate harness policy and hooks";
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.toolgate;
      description = "toolgate package";
    };
    mode = lib.mkOption {
      type = lib.types.enum [
        "enforce"
        "observe"
      ];
      default = "enforce";
    };
    policy = lib.mkOption {
      type = lib.types.attrsOf lib.types.anything;
      default = { };
      description = "Extra policy.toml keys merged over defaults";
    };
    harnesses = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [
        "cursor"
        "muse"
        "opencode"
        "pi"
      ];
    };
    mcp.enable = lib.mkEnableOption "Register toolgate MCP stdio server";
  };

  config = lib.mkIf cfg.enable {
    xdg.configFile."toolgate/policy.toml".text =
      let
        base = {
          mode = cfg.mode;
        };
        merged = base // cfg.policy;
      in
      pkgs.formats.toml { } .generate "" merged;

    home.packages = [ cfg.package ];

    xdg.configFile."opencode/plugins/toolgate-hook.mjs".source =
      ../../adapters/opencode/toolgate-hook.mjs;

    xdg.configFile."pi/extensions/toolgate-hook.mjs".source =
      ../../adapters/pi/toolgate-hook.mjs;

    home.activation.toolgateMergeHooks = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
      cursor_patch=${pkgs.writeText "toolgate-cursor-hooks.json" (builtins.toJSON {
        version = 1;
        hooks = {
          preToolUse = [
            {
              matcher = "Read|Shell|Grep|Glob";
              command = "${toolgateBin} hook --harness cursor --event preToolUse";
            }
          ];
          postToolUse = [
            {
              command = "${toolgateBin} hook --harness cursor --event postToolUse";
            }
          ];
          afterShellExecution = [
            {
              command = "${toolgateBin} hook --harness cursor --event afterShellExecution";
            }
          ];
          afterMCPExecution = [
            {
              command = "${toolgateBin} hook --harness cursor --event afterMCPExecution";
            }
          ];
        };
      })}
      muse_patch=${pkgs.writeText "toolgate-muse-hooks.json" (builtins.toJSON {
        hooks = {
          PreToolUse = [
            {
              matcher = "Read|Shell|Grep|Glob|Bash";
              hooks = [
                {
                  type = "command";
                  command = "${toolgateBin} hook --harness muse --event PreToolUse";
                }
              ];
            }
          ];
          PostToolUse = [
            {
              hooks = [
                {
                  type = "command";
                  command = "${toolgateBin} hook --harness muse --event PostToolUse";
                }
              ];
            }
          ];
        };
      })}
      ${mergeHooks} "$HOME/.cursor/hooks.json" "$cursor_patch"
      mkdir -p "$HOME/.config/muse"
      ${mergeHooks} "$HOME/.config/muse/settings.json" "$muse_patch"
    '';

    home.sessionVariables = lib.mkIf cfg.mcp.enable {
      TOOLGATE_POLICY = cfg.mode;
    };
  };
}
