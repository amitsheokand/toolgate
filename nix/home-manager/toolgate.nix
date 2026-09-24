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
    for patch in "$@"; do
      ${pkgs.jq}/bin/jq -s '.[0] * .[1]' "$tmp" "$patch" > "$tmp.new"
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

    xdg.configFile."opencode/toolgate-hook.mjs".source =
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
              matcher = "MCP:*";
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
              matcher = "Read|Shell|Grep|Glob";
              command = "${toolgateBin} hook --harness muse --event PreToolUse";
            }
          ];
          PostToolUse = [
            {
              matcher = "MCP:*";
              command = "${toolgateBin} hook --harness muse --event PostToolUse";
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
