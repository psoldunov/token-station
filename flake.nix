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
          token-station = pkgs.symlinkJoin {
            name = "token-station-${native.daemon.package.version}";
            paths = [
              native.daemon.package
              native.frontends.plasmoid
              native.frontends.gnome-extension
            ];
            meta = native.daemon.package.meta;
          };
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
          nix-fmt = pkgs.runCommand "nix-fmt-check" { nativeBuildInputs = [ pkgs.nixfmt ]; } ''
            nixfmt --check ${./flake.nix} ${./nix}/*.nix
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
