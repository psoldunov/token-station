# home-manager module: `programs.token-station`.
self:
{
  config,
  lib,
  pkgs,
  options,
  ...
}:
let
  cfg = config.programs.token-station;
  tomlFormat = pkgs.formats.toml { };
  busName = "dev.soldunov.TokenStation";
  gnomeUuid = "token-station@soldunov.dev";
  # systemd user services start with a minimal PATH; the daemon also probes these,
  # but listing them keeps `claude`/`codex` discovery explicit.
  searchPath = lib.concatStringsSep ":" [
    "${config.home.profileDirectory}/bin"
    "/etc/profiles/per-user/${config.home.username}/bin"
    "/run/current-system/sw/bin"
    "${config.home.homeDirectory}/.local/bin"
  ];
  statuslineCommand =
    "${lib.getExe cfg.package} statusline"
    + lib.optionalString (cfg.claudeStatusline.wrap != null) " --wrap ${lib.escapeShellArg cfg.claudeStatusline.wrap}";
in
{
  options.programs.token-station = {
    enable = lib.mkEnableOption "Token Station, a tray monitor for Claude Code and Codex plan usage";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "token-station.packages.\${system}.default";
      description = "Package providing the daemon, the Plasma applet and the GNOME extension.";
    };

    settings = lib.mkOption {
      inherit (tomlFormat) type;
      default = { };
      example = lib.literalExpression ''
        {
          alerts = { warning_percent = 75; critical_percent = 90; };
          codex.enabled = false;
        }
      '';
      description = ''
        Contents of {file}`$XDG_CONFIG_HOME/token-station/config.toml`. When set, the
        file is read-only and settings changed from the applet/extension are not saved.
      '';
    };

    gnome.enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Enable the GNOME Shell extension by adding `${gnomeUuid}` to
        `org/gnome/shell/enabled-extensions` (with `mkDefault`: if you set that list
        yourself, include the UUID there).
      '';
    };

    claudeStatusline = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Point Claude Code's statusLine at `token-station statusline` so live plan usage
          reaches the daemon while Claude Code runs. Requires `programs.claude-code`.
        '';
      };
      wrap = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "bash ~/.claude/statusline.sh";
        description = "Existing statusline command to keep rendering (its output is passed through).";
      };
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        home.packages = [ cfg.package ];

        xdg.configFile."token-station/config.toml" = lib.mkIf (cfg.settings != { }) {
          source = tomlFormat.generate "token-station-config.toml" cfg.settings;
        };

        # Route D-Bus activation through systemd so the daemon is supervised.
        xdg.dataFile."dbus-1/services/${busName}.service".text = ''
          [D-BUS Service]
          Name=${busName}
          Exec=${lib.getExe cfg.package} daemon
          SystemdService=token-station.service
        '';

        systemd.user.services.token-station = {
          Unit = {
            Description = "Token Station usage daemon";
            Documentation = [ "https://github.com/psoldunov/token-station" ];
            PartOf = [ "graphical-session.target" ];
            After = [ "graphical-session.target" ];
          };
          Service = {
            Type = "dbus";
            BusName = busName;
            ExecStart = "${lib.getExe cfg.package} daemon";
            Restart = "on-failure";
            RestartSec = 5;
            Environment = [ "PATH=${searchPath}" ];
          };
          Install.WantedBy = [ "graphical-session.target" ];
        };

        dconf.settings = lib.mkIf cfg.gnome.enable {
          "org/gnome/shell".enabled-extensions = lib.mkDefault [ gnomeUuid ];
        };
      }
      (lib.optionalAttrs (options.programs ? claude-code) {
        programs.claude-code.settings.statusLine = lib.mkIf cfg.claudeStatusline.enable {
          type = "command";
          command = statuslineCommand;
        };
      })
    ]
  );
}
