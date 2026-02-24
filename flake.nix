{
  description = "Browser testing on Antithesis";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  nixConfig = {
    extra-substituters = "https://bombadil.cachix.org";
    extra-trusted-public-keys = "bombadil.cachix.org-1:6L4epM9zwhEcAwouNgBa8ENtsgLNfedtQgqtdnQhZiM=";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      crane,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = (
          import nixpkgs {
            inherit system;
            overlays = [ ];
          }
        );
        craneLib = crane.mkLib pkgs;
        craneLibStatic = crane.mkLib pkgs.pkgsCross.musl64;
        craneLibAarch64 = crane.mkLib pkgs.pkgsCross.aarch64-multiplatform-musl;
        bombadil = pkgs.callPackage ./nix/default.nix { inherit craneLib craneLibStatic; };
        bombadilAarch64 = pkgs.callPackage ./nix/default.nix {
          inherit craneLib;
          craneLibStatic = craneLibAarch64;
          cargoTarget = "aarch64-unknown-linux-musl";
        };
      in
      {
        packages = {
          default = bombadil.bin;
          types = bombadil.types;
          manual = pkgs.callPackage ./docs/manual/default.nix { };
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          aarch64-linux = bombadilAarch64.bin;
          docker = pkgs.callPackage ./nix/docker.nix { bombadil = self.packages.${system}.default; };
        };

        apps = {
          default = {
            type = "app";
            program = "${self.packages.${system}.default}/bin/bombadil";
            meta = self.packages.${system}.default.meta;
          };
        };

        checks = {

          inherit (bombadil) clippy fmt types;
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          inherit (bombadil) tests;
        };

        devShells = {
          default = pkgs.mkShell (
            {
              CARGO_INSTALL_ROOT = "${toString ./.}/.cargo";
              inputsFrom = [ self.packages.${system}.default ];
              buildInputs =
                with pkgs;
                [
                  # Rust
                  cargo
                  rustc
                  rust-analyzer
                  rustfmt
                  crate2nix
                  cargo-insta
                  clippy

                  # Nix
                  nil

                  # TS/JS
                  typescript
                  typescript-language-server
                  esbuild
                  bun
                  biome
                ]
                ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
                  # Runtime
                  pkgs.chromium
                ];
            }
            // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
              # override how chromiumoxide finds the chromium executable
              CHROME = pkgs.lib.getExe pkgs.chromium;
            }
          );

          manual = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.manual ];
            buildInputs = with pkgs; [
              watchexec
              nodePackages.browser-sync
              nodePackages.concurrently
            ];
            OSFONTDIR = "${pkgs.ibm-plex}/share/fonts/opentype";
          };
        };
      }
    );
}
