{
  description = "Property-based testing for web UIs";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
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
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = (
          import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          }
        );
        rustToolchainWasm = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ];
        };
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchainWasm;
        craneLibStatic = (crane.mkLib pkgs.pkgsCross.musl64).overrideToolchain (
          p:
          p.rust-bin.stable.latest.default.override {
            targets = [
              "wasm32-unknown-unknown"
              "x86_64-unknown-linux-musl"
            ];
          }
        );
        craneLibAarch64 = (crane.mkLib pkgs.pkgsCross.aarch64-multiplatform-musl).overrideToolchain (
          p:
          p.rust-bin.stable.latest.default.override {
            targets = [
              "wasm32-unknown-unknown"
              "aarch64-unknown-linux-musl"
            ];
          }
        );
        # Pinned to match `GHOSTTY_COMMIT` in libghostty-vt-sys's build.rs at
        # the `libghostty-vt` rev used by `lib/bombadil-terminal/Cargo.toml`.
        # Bump these together when updating libghostty-vt.
        ghosttySrc = pkgs.fetchFromGitHub {
          owner = "ghostty-org";
          repo = "ghostty";
          rev = "6590196661f769dd8f2b3e85d6c98262c4ec5b3b";
          sha256 = "0bxq9pv568zr6ns5szmhg18id7f68mbkhqaygm641c3cw1df0w8w";
        };
        bombadil = pkgs.callPackage ./lib/nix/default.nix {
          inherit craneLib craneLibStatic ghosttySrc;
        };
        bombadilAarch64 = pkgs.callPackage ./lib/nix/default.nix {
          inherit craneLib ghosttySrc;
          craneLibStatic = craneLibAarch64;
          cargoTarget = "aarch64-unknown-linux-musl";
        };
      in
      {
        packages = {
          default = bombadil.bin;
          npm-package = bombadil.npm-package;
          manual = pkgs.callPackage ./docs/manual/default.nix { };
          release = pkgs.callPackage ./lib/release/default.nix { };
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          aarch64-linux = bombadilAarch64.bin;
          docker = pkgs.callPackage ./lib/nix/docker.nix { bombadil = self.packages.${system}.default; };
        };

        apps = {
          default = {
            type = "app";
            program = "${self.packages.${system}.default}/bin/bombadil";
            meta = self.packages.${system}.default.meta;
          };
        };

        checks = {
          inherit (bombadil) clippy fmt npm-package;
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          inherit (bombadil) tests;
        };

        devShells = {
          default = pkgs.mkShell (
            {
              shellHook = ''
                export CC=${pkgs.clang}/bin/clang
                export CXX=${pkgs.clang}/bin/clang++
              '';
              CARGO_INSTALL_ROOT = "${toString ./.}/.cargo";
              inputsFrom = [ self.packages.${system}.default ];
              # nativeBuildInputs takes priority over inputsFrom in
              # PATH, so rustToolchainWasm shadows crane's toolchain.
              nativeBuildInputs = [ rustToolchainWasm ];
              buildInputs =
                with pkgs;
                [
                  # Rust
                  rust-analyzer
                  crate2nix
                  cargo-insta

                  # Nix
                  nil

                  # For bombadil-terminal. zig_0_15 / pkg-config come in via
                  # `inputsFrom = [ self.packages.${system}.default ]`; adding
                  # them again here re-sources zig's setup-hook and trips its
                  # readonly `zigDefaultCpuFlag` guard.
                  cmake
                  clang

                  # TS/JS
                  typescript
                  typescript-language-server
                  esbuild
                  bun
                  biome

                  # WASM/Inspect UI
                  trunk
                  wasm-bindgen-cli
                  binaryen

                  # Release automation
                  self.packages.${system}.release
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
              browser-sync
              concurrently
            ];
            OSFONTDIR = "${pkgs.ibm-plex}/share/fonts/opentype";
          };

          release = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.release ];
          };
        };
      }
    );
}
