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
  plasmoidId = "dev.soldunov.tokenstation";
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
    + lib.optionalString (
      cfg.claudeStatusline.wrap != null
    ) " --wrap ${lib.escapeShellArg cfg.claudeStatusline.wrap}";
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
        `org/gnome/shell/enabled-extensions`. home-manager concatenates GVariant
        array definitions, so your own `enabled-extensions` list is kept and the
        UUID is appended to it.
      '';
    };

    claudeStatusline = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Point Claude Code's statusLine at `token-station statusline` so live plan usage
          reaches the daemon while Claude Code runs. Requires `programs.claude-code`.

          This replaces an existing `programs.claude-code.settings.statusLine`
          definition (it is set at `lib.mkOverride 90`); put the command you had
          in {option}`claudeStatusline.wrap` to keep its output.
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
            # 75 = another daemon already owns the bus name; restarting cannot help.
            RestartPreventExitStatus = 75;
            NoNewPrivileges = true;
            Environment = [ "PATH=${searchPath}" ];
          };
          Install.WantedBy = [ "graphical-session.target" ];
        };

        # Plasma's system tray looks for applets when plasmashell starts, and
        # after that only when KPackage announces an install over D-Bus. A
        # switch installs the applet without that announcement, so a running
        # session would not show it until the next login. Announce it whenever
        # this generation adds or changes it; the tray then adds the applet, or
        # restarts it if it is already there. Nothing listens outside Plasma.
        home.activation.tokenStationPlasmoid =
          lib.hm.dag.entryAfter [ "installPackages" "linkGeneration" ]
            ''
              tokenStationAnnouncePlasmoid() {
                local rel=home-path/share/plasma/plasmoids/${plasmoidId}
                local new old="" bus socket
                new=$(readlink -e "$newGenPath/$rel") || return 0
                if [[ -v oldGenPath && -e "$oldGenPath/$rel" ]]; then
                  old=$(readlink -e "$oldGenPath/$rel")
                fi
                [[ $new != "$old" ]] || return 0

                # Under the NixOS module the switch runs from a system service
                # with no session bus in its environment.
                bus=''${DBUS_SESSION_BUS_ADDRESS:-}
                if [[ -z $bus ]]; then
                  socket=''${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/bus
                  [[ -S $socket ]] || return 0
                  bus=unix:path=$socket
                fi

                if ! run ${lib.getExe' pkgs.dbus "dbus-send"} --bus="$bus" --type=signal \
                  /KPackage/Plasma/Applet org.kde.plasma.kpackage.packageInstalled \
                  string:${plasmoidId}; then
                  warnEcho "Could not announce the Token Station applet to Plasma; it appears after the next login."
                fi
              }
              tokenStationAnnouncePlasmoid
            '';

        # A plain definition: home-manager's GVariant arrays concatenate, so a
        # user-defined enabled-extensions list keeps its entries and gains this one.
        dconf.settings = lib.mkIf cfg.gnome.enable {
          "org/gnome/shell".enabled-extensions = [ gnomeUuid ];
        };
      }
      (lib.optionalAttrs (options.programs ? claude-code) {
        # Turning `claudeStatusline.enable` on is an explicit request to replace
        # whatever statusLine is configured, so it outranks a normal definition
        # (`wrap` keeps the previous command rendering). A user who wants the
        # opposite can still win with `lib.mkForce`.
        programs.claude-code.settings.statusLine = lib.mkIf cfg.claudeStatusline.enable (
          lib.mkOverride 90 {
            type = "command";
            command = statuslineCommand;
          }
        );
      })
    ]
  );
}
