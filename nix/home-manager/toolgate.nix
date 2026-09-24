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
  toolgateBasename = builtins.baseNameOf toolgateBin;
  mergeHooks = import ./merge-hooks.nix {
    inherit pkgs toolgateBasename;
  };
  substituteHook = src:
    pkgs.replaceVars src {
      TOOLGATE_BIN = toolgateBin;
    };
  opencodeHook = substituteHook ../../adapters/opencode/toolgate-hook.mjs;
  piHook = substituteHook ../../adapters/pi/toolgate-hook.ts;
  hasHarness = h: lib.elem h cfg.harnesses;
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
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      xdg.configFile."toolgate/policy.toml".text =
        let
          base = {
            mode = cfg.mode;
          };
          merged = base // cfg.policy;
        in
        pkgs.formats.toml { } .generate "" merged;

      home.packages = [ cfg.package ];
    }
    (lib.mkIf (hasHarness "opencode") {
      xdg.configFile."opencode/plugins/toolgate-hook.mjs".source = opencodeHook;
    })
    (lib.mkIf (hasHarness "pi") {
      home.file.".pi/agent/extensions/toolgate-hook.ts".source = piHook;
    })
    {
      home.activation.toolgateMergeHooks = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
        cursor_patch=${pkgs.writeText "toolgate-cursor-hooks.json" (
          if hasHarness "cursor" then
            builtins.toJSON {
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
            }
          else
            builtins.toJSON {
              version = 1;
              hooks = { };
            }
        )}
        ${mergeHooks} "$HOME/.cursor/hooks.json" "$cursor_patch"

        muse_patch=${pkgs.writeText "toolgate-muse-hooks.json" (
          if hasHarness "muse" then
            builtins.toJSON {
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
            }
          else
            builtins.toJSON {
              hooks = { };
            }
        )}
        mkdir -p "$HOME/.config/muse"
        ${mergeHooks} "$HOME/.config/muse/settings.json" "$muse_patch"
      '';
    }
  ]);
}
