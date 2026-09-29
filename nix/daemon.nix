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
  # Dependencies for one cargo pass each, rather than crane's default of check,
  # build and test in a row: every consumer waits only on its own set, and the
  # static AppImage build compiles nothing it does not link.
  depsFor =
    args: suffix: command:
    craneLib.buildDepsOnly (
      args
      // {
        pname = "${commonArgs.pname}${suffix}";
        buildPhaseCargoCommand = "cargoWithProfile ${command} ${commonArgs.cargoExtraArgs}";
        doCheck = false;
      }
    );
  # Tests build in cargo's own test profile, as `cargo test` does locally. The
  # release profile's thin LTO with one codegen unit re-optimises every test
  # binary, which made the test check take 12 minutes in CI.
  testArgs = commonArgs // {
    CARGO_PROFILE = "";
  };
  # The package links release builds; clippy reads check metadata.
  cargoArtifacts = depsFor commonArgs "" "build";
  checkArtifacts = depsFor commonArgs "-check" "check --all-targets";
  testArtifacts = depsFor testArgs "-test" "test --no-run";
in
{
  inherit
    commonArgs
    checkArtifacts
    testArgs
    testArtifacts
    ;

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
