# NixOS module: `programs.token-station` (system-wide install for all users).
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.token-station;
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
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    services.dbus.packages = [ cfg.package ];
    systemd.packages = [ cfg.package ];
    systemd.user.services.token-station.wantedBy = [ "graphical-session.target" ];
  };
}
