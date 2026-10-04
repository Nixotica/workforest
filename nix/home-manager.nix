# The Home Manager module: `programs.workforest`. The flake passes itself in,
# for the package to default to.
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.workforest;
  toml = pkgs.formats.toml { };
  exe = lib.getExe cfg.package;
  shellOption =
    shell:
    lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Load `wfcd`, which cds into a forest or one of its trees, in ${shell}.";
    };
in
{
  options.programs.workforest = {
    enable = lib.mkEnableOption "workforest, isolated git worktrees for every piece of work";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "workforest.packages.\${system}.default";
      description = "The workforest package to install.";
    };

    settings = lib.mkOption {
      inherit (toml) type;
      default = { };
      example = lib.literalExpression ''
        {
          repos = "~/code";
          cache.link_min = 65536;
        }
      '';
      description = ''
        Written to {file}`$XDG_CONFIG_HOME/workforest/config.toml`; see
        "Configuration" in workforest's README. Set `repos` here rather than
        with `workforest setup`, which can't write a file Home Manager manages.
      '';
    };

    installClaudeSkill = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Link the package's agent skill into {file}`~/.claude/skills/workforest`,
        so that it updates with the CLI. Leave it off when the workforest Claude
        Code plugin supplies the skill.
      '';
    };

    enableBashIntegration = shellOption "bash";
    enableZshIntegration = shellOption "zsh";
    enableFishIntegration = shellOption "fish";

    fire = {
      enable = lib.mkEnableOption ''
        a scheduled `workforest fire`, which burns every forest whose work has
        landed. It runs as a systemd user timer, so on Linux only'';

      frequency = lib.mkOption {
        type = lib.types.str;
        default = "daily";
        description = "When it runs, as a systemd calendar event.";
      };

      flags = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ "--yes" ];
        example = [
          "--yes"
          "--delete-branches"
        ];
        description = ''
          Flags for `workforest fire`. Without `--yes` it only reports, in the
          service's journal, what it would burn.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        home.packages = [ cfg.package ];

        xdg.configFile."workforest/config.toml" = lib.mkIf (cfg.settings != { }) {
          source = toml.generate "workforest-config.toml" cfg.settings;
        };

        home.file = lib.mkIf cfg.installClaudeSkill {
          ".claude/skills/workforest".source = "${cfg.package}/share/workforest/skills/workforest";
        };

        programs.bash.initExtra = lib.mkIf cfg.enableBashIntegration ''
          eval "$(${exe} shell-init bash)"
        '';
        programs.zsh.initContent = lib.mkIf cfg.enableZshIntegration ''
          eval "$(${exe} shell-init zsh)"
        '';
        programs.fish.interactiveShellInit = lib.mkIf cfg.enableFishIntegration ''
          ${exe} shell-init fish | source
        '';
      }

      (lib.mkIf cfg.fire.enable {
        assertions = [
          {
            assertion = pkgs.stdenv.hostPlatform.isLinux;
            message = "programs.workforest.fire runs as a systemd user timer, which needs Linux.";
          }
        ];

        systemd.user.services.workforest-fire = {
          Unit.Description = "Burn every forest whose work has landed";
          Service = {
            Type = "oneshot";
            # fire fetches each repo's remote; one it can't reach, as with an
            # SSH remote needing an agent, is judged as last fetched.
            Environment = "PATH=${
              lib.makeBinPath [
                pkgs.git
                pkgs.openssh
              ]
            }";
            ExecStart = "${exe} fire ${lib.escapeShellArgs cfg.fire.flags}";
          };
        };

        systemd.user.timers.workforest-fire = {
          Unit.Description = "Burn every forest whose work has landed, ${cfg.fire.frequency}";
          Timer = {
            OnCalendar = cfg.fire.frequency;
            Persistent = true;
          };
          Install.WantedBy = [ "timers.target" ];
        };
      })
    ]
  );
}
