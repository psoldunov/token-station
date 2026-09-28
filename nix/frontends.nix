# Shell-native front ends: the Plasma applet and the GNOME Shell extension.
{
  lib,
  stdenvNoCC,
  glib,
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
    dontBuild = true;
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
    src = ../frontends/gnome + "/${gnomeUuid}";
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
