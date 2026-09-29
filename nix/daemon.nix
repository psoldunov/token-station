# The Rust daemon + CLI, plus the session integration files it ships
# (systemd user unit, D-Bus activation file, desktop entries, icons).
{
  lib,
  craneLib,
  dbus,
}:
let
  root = ../.;
  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../data
    ];
  };
  cargoToml = lib.importTOML ../Cargo.toml;
  commonArgs = {
    inherit src;
    pname = "token-station";
    version = cargoToml.workspace.package.version;
    strictDeps = true;
    cargoExtraArgs = "--locked";
    # D-Bus integration tests start a private dbus-daemon.
    nativeCheckInputs = [ dbus ];
  };
  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
in
{
  inherit commonArgs cargoArtifacts;

  package = craneLib.buildPackage (
    commonArgs
    // {
      inherit cargoArtifacts;
      # Tests run in `checks`; the package build stays fast.
      doCheck = false;
      postInstall = ''
        substitute data/systemd/token-station.service.in \
          $out/lib/systemd/user/token-station.service --subst-var-by bindir $out/bin
        substitute data/dbus/dev.soldunov.TokenStation.service.in \
          $out/share/dbus-1/services/dev.soldunov.TokenStation.service --subst-var-by bindir $out/bin
        install -Dm644 -t $out/share/applications data/applications/dev.soldunov.TokenStation.desktop
        install -Dm644 -t $out/etc/xdg/autostart data/applications/dev.soldunov.TokenStation.Tray.desktop
        install -Dm644 -t $out/share/icons/hicolor/scalable/apps \
          data/icons/hicolor/scalable/apps/dev.soldunov.TokenStation.svg
        install -Dm644 -t $out/share/icons/hicolor/symbolic/apps \
          data/icons/hicolor/symbolic/apps/dev.soldunov.TokenStation-symbolic.svg
      '';
      preInstall = ''
        mkdir -p $out/lib/systemd/user $out/share/dbus-1/services
      '';
      meta = {
        description = "Native Linux tray monitor for Claude Code and Codex plan usage";
        homepage = "https://github.com/psoldunov/token-station";
        license = lib.licenses.mit;
        mainProgram = "token-station";
        platforms = lib.platforms.linux;
      };
    }
  );
}
