# Portable AppImage: static musl daemon + front-end payloads for `token-station setup`.
#
# Assembled like appimagetool does it: a squashfs image appended to the static
# type2 runtime (no FUSE 2 dependency, no network access during the build).
{
  lib,
  stdenvNoCC,
  fetchurl,
  squashfsTools,
  daemon,
  plasmoid,
  gnome-extension,
}:
let
  arch = stdenvNoCC.hostPlatform.parsed.cpu.name;
  runtimes = {
    x86_64 = fetchurl {
      url = "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64";
      hash = "sha256-L8qLRDySUQ8Ug6iD9gBhrQm0a5eLJjHIB82HOkfsJg0=";
    };
    aarch64 = fetchurl {
      url = "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-aarch64";
      hash = "sha256-AMvfz5F8xsD/bTNH1Z4Moff0Wm3xpCig1tinhmTYdEQ=";
    };
  };
  appId = "dev.soldunov.TokenStation";
in
stdenvNoCC.mkDerivation {
  pname = "token-station-appimage";
  inherit (daemon) version;
  dontUnpack = true;
  nativeBuildInputs = [ squashfsTools ];

  buildPhase = ''
    runHook preBuild
    appdir=$PWD/AppDir
    share=$appdir/usr/share
    mkdir -p $appdir/usr/bin $share/token-station/integrations
    install -m755 ${daemon}/bin/token-station $appdir/usr/bin/token-station
    ln -s usr/bin/token-station $appdir/AppRun
    cp -r ${plasmoid}/share/plasma/plasmoids/dev.soldunov.tokenstation \
      $share/token-station/integrations/plasma
    cp -r ${gnome-extension}/share/gnome-shell/extensions/token-station@soldunov.dev \
      $share/token-station/integrations/gnome
    cp -r ${daemon}/share/icons ${daemon}/share/applications $share/
    cp ${daemon}/share/applications/${appId}.desktop $appdir/${appId}.desktop
    cp ${daemon}/share/icons/hicolor/scalable/apps/${appId}.svg $appdir/${appId}.svg
    ln -s ${appId}.svg $appdir/.DirIcon
    chmod -R u+w $appdir
    mksquashfs $appdir app.squashfs -all-root -noappend -no-xattrs -comp zstd -quiet
    cat ${runtimes.${arch}} app.squashfs > TokenStation-${arch}.AppImage
    chmod +x TokenStation-${arch}.AppImage
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    install -Dm755 TokenStation-${arch}.AppImage $out/TokenStation-${arch}.AppImage
    runHook postInstall
  '';

  meta = {
    description = "Token Station as a portable AppImage";
    license = lib.licenses.mit;
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
  };
}
