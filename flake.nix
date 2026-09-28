{
  description = "Token Station: native Linux tray monitor for Claude Code and Codex usage";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Everything buildable for one package set (native or static musl).
      mkBuild =
        pkgs:
        let
          craneLib = crane.mkLib pkgs;
          daemon = pkgs.callPackage ./nix/daemon.nix { inherit craneLib; };
          frontends = pkgs.callPackage ./nix/frontends.nix { version = daemon.package.version; };
        in
        {
          inherit craneLib daemon frontends;
        };
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          native = mkBuild pkgs;
          static = mkBuild pkgs.pkgsStatic;
          # One tree holding real directories and files, not a symlink farm:
          # KPackage refuses a plasmoid whose contents are symlinks ("Path
          # traversal attempt detected"), so the applet would never load.
          token-station =
            pkgs.runCommand "token-station-${native.daemon.package.version}"
              {
                inherit (native.daemon.package) meta;
                passthru = {
                  daemon = native.daemon.package;
                  inherit (native.frontends) plasmoid gnome-extension;
                };
              }
              ''
                mkdir -p $out
                for tree in \
                  ${native.daemon.package} \
                  ${native.frontends.plasmoid} \
                  ${native.frontends.gnome-extension}
                do
                  cp -rL "$tree"/. $out/
                  chmod -R u+w $out
                done
              '';
        in
        {
          default = token-station;
          inherit token-station;
          daemon = native.daemon.package;
          daemon-static = static.daemon.package;
          inherit (native.frontends) plasmoid gnome-extension;
          appimage = pkgs.callPackage ./nix/appimage.nix {
            daemon = static.daemon.package;
            inherit (native.frontends) plasmoid gnome-extension;
          };
        }
      );

      checks = forAllSystems (
        pkgs:
        let
          inherit (mkBuild pkgs) craneLib daemon;
          args = daemon.commonArgs // {
            inherit (daemon) cargoArtifacts;
          };
        in
        {
          build = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
          clippy = craneLib.cargoClippy (
            args
            // {
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            }
          );
          test = craneLib.cargoTest args;
          fmt = craneLib.cargoFmt { inherit (daemon.commonArgs) src; };
          # The applet as installed: KPackage has to find it by plugin id and
          # hand back a real path. A symlinked package still "shows", but with
          # an empty path, which is exactly how it fails to load in the shell.
          plasmoid-loads =
            pkgs.runCommand "token-station-plasmoid-loads"
              {
                nativeBuildInputs = [
                  pkgs.kdePackages.kpackage
                  pkgs.jq
                ];
              }
              ''
                share=${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/share
                package=$share/plasma/plasmoids/dev.soldunov.tokenstation

                if [ -n "$(find "$package" -type l -print -quit)" ]; then
                  echo "the installed plasmoid contains symlinks; KPackage rejects those:" >&2
                  find "$package" -type l >&2
                  exit 1
                fi
                jq -e '.KPlugin.Id == "dev.soldunov.tokenstation"' "$package/metadata.json" > /dev/null

                export HOME=$TMPDIR
                export XDG_DATA_DIRS=$share
                export QT_QPA_PLATFORM=offscreen
                # The Plasma/Applet package structure plugin ships with libplasma.
                export QT_PLUGIN_PATH=${pkgs.kdePackages.libplasma}/${pkgs.qt6.qtbase.qtPluginPrefix}
                kpackagetool6 --type Plasma/Applet --show dev.soldunov.tokenstation | tee info.txt
                grep -q "Plugin     : dev.soldunov.tokenstation" info.txt
                # An empty path is KPackage having refused the package contents.
                grep -qE "^  Path       : .*/dev.soldunov.tokenstation/?$" info.txt
                touch $out
              '';
          # Boots GNOME in a VM, enables the extension against a fixture daemon,
          # asserts it loaded cleanly and renders the menu (needs KVM).
          gnome-vm = import ./nix/tests/gnome.nix {
            inherit pkgs;
            tokenStation = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
            fixture = ./data/fixtures/snapshot-near-limit.json;
          };
          nix-fmt = pkgs.runCommand "nix-fmt-check" { nativeBuildInputs = [ pkgs.nixfmt ]; } ''
            nixfmt --check ${./flake.nix} ${./nix}/*.nix ${./nix/tests}/*.nix
            touch $out
          '';
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            cargo-llvm-cov
            pkg-config
            sqlite
            dbus
            jq
            squashfsTools
            kdePackages.plasma-sdk
            kdePackages.kpackage
            kdePackages.qtdeclarative
            gjs
            glib
            nixfmt
          ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          # cargo-llvm-cov needs the LLVM tools matching rustc's LLVM.
          LLVM_COV = "${pkgs.rustc.llvm}/bin/llvm-cov";
          LLVM_PROFDATA = "${pkgs.rustc.llvm}/bin/llvm-profdata";
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);

      overlays.default = final: _prev: {
        token-station = self.packages.${final.stdenv.hostPlatform.system}.default;
      };

      homeManagerModules.default = import ./nix/hm-module.nix self;
      nixosModules.default = import ./nix/nixos-module.nix self;
    };
}
