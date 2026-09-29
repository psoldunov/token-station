# Shell-native front ends: the Plasma applet and the GNOME Shell extension.
{
  lib,
  stdenvNoCC,
  glib,
  jq,
  version,
}:
let
  plasmoidId = "dev.soldunov.tokenstation";
  gnomeUuid = "token-station@soldunov.dev";
  meta = {
    homepage = "https://github.com/psoldunov/token-station";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
in
{
  plasmoid = stdenvNoCC.mkDerivation {
    pname = "token-station-plasmoid";
    inherit version;
    src = ../frontends/plasma + "/${plasmoidId}";
    nativeBuildInputs = [ jq ];
    # KPlugin.Version is what Plasma shows in "Add Widgets"; stamp it from the
    # workspace version rather than trusting the checked-in copy.
    buildPhase = ''
      runHook preBuild
      jq --arg version ${lib.escapeShellArg version} \
        '.KPlugin.Version = $version' metadata.json > metadata.stamped.json
      mv metadata.stamped.json metadata.json
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out/share/plasma/plasmoids/${plasmoidId}
      cp -r . $out/share/plasma/plasmoids/${plasmoidId}/
      runHook postInstall
    '';
    meta = meta // {
      description = "Token Station system tray applet for KDE Plasma 6";
    };
  };

  gnome-extension = stdenvNoCC.mkDerivation {
    pname = "gnome-shell-extension-token-station";
    inherit version;
    # A store path name cannot contain `@`, so name the source explicitly.
    src = builtins.path {
      name = "token-station-gnome-extension-src";
      path = ../frontends/gnome + "/${gnomeUuid}";
    };
    nativeBuildInputs = [ glib ];
    buildPhase = ''
      runHook preBuild
      if [ -d schemas ]; then glib-compile-schemas --strict schemas; fi
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out/share/gnome-shell/extensions/${gnomeUuid}
      cp -r . $out/share/gnome-shell/extensions/${gnomeUuid}/
      runHook postInstall
    '';
    passthru.extensionUuid = gnomeUuid;
    meta = meta // {
      description = "Token Station top-bar indicator for GNOME Shell";
    };
  };
}
