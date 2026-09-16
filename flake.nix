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
    hegel = {
      url = "github:hegeldev/hegel-rust?dir=nix&ref=v0.45.2";
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
      hegel,
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
        # Per-host musl cross targets so each native runner (x86_64-linux,
        # aarch64-linux) builds its own static binary without cross-compiling.
        muslCrossPkgs =
          if pkgs.stdenv.hostPlatform.isAarch64 then
            pkgs.pkgsCross.aarch64-multiplatform-musl
          else
            pkgs.pkgsCross.musl64;
        muslTarget =
          if pkgs.stdenv.hostPlatform.isAarch64 then
            "aarch64-unknown-linux-musl"
          else
            "x86_64-unknown-linux-musl";
        craneLibStatic = (crane.mkLib muslCrossPkgs).overrideToolchain (
          p:
          p.rust-bin.stable.latest.default.override {
            targets = [
              "wasm32-unknown-unknown"
              muslTarget
            ];
          }
        );
        # Pinned to match `GHOSTTY_COMMIT` in libghostty-vt-sys's build.rs at
        # the `libghostty-vt` rev used by `lib/bombadil-terminal/Cargo.toml`.
        # Bump these together when updating libghostty-vt.
        ghosttySrc = pkgs.fetchFromGitHub {
          owner = "ghostty-org";
          repo = "ghostty";
          rev = "a887df42c56f6de86c0fe6da9c4eeca37931e083";
          sha256 = "sha256-1Zz65SCk3rkJ9+Q0MmyNOTNiDSLBRIHRd3IvFM4iNXw=";
        };
        # Override postInstall to keep the native `libhegel_c.{so,dylib}`
        # name (hegeltest's loader wants that, not upstream's `libhegel.so`).
        libhegelC = (hegel.lib.mkLibhegel { inherit pkgs; }).overrideAttrs (_: {
          postInstall =
            let
              ext = if pkgs.stdenv.hostPlatform.isDarwin then "dylib" else "so";
            in
            ''
              mkdir -p $out/lib
              cp "$(find target -name 'libhegel_c.${ext}' -path '*/release/*' | head -n1)" \
                $out/lib/libhegel_c.${ext}
            '';
        });
        bombadil = pkgs.callPackage ./lib/nix/default.nix {
          inherit craneLib craneLibStatic ghosttySrc libhegelC;
        };
      in
      {
        packages = {
          default = bombadil.bin;
          npm-package = bombadil.npm-package;
          manual = pkgs.callPackage ./docs/manual/default.nix {
            inherit (bombadil) npm-package typescript-reference;
          };
          release = pkgs.callPackage ./lib/release/default.nix { };
          nix-build-push = pkgs.callPackage ./lib/nix/cachix-push.nix { };
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          docker = pkgs.callPackage ./lib/nix/docker.nix { bombadil = bombadil.bin-dynamic; };
        };

        apps = {
          default = {
            type = "app";
            program = "${self.packages.${system}.default}/bin/bombadil";
            meta = self.packages.${system}.default.meta;
          };
          nix-build-push = {
            type = "app";
            program = "${self.packages.${system}.nix-build-push}/bin/nix-build-push";
          };
          hotpath = {
            type = "app";
            program = "${pkgs.callPackage ./lib/nix/cargo-hotpath.nix { }}/bin/hotpath";
          };
        };

        checks = {
          inherit (bombadil) clippy fmt npm-package;
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          inherit (bombadil) tests-unit tests-browser;
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
                  lldb

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
                  bun
                  biome

                  # WASM/Inspect UI
                  trunk
                  wasm-bindgen-cli
                  binaryen

                  # Release automation
                  self.packages.${system}.release

                  # Pre-warm cachix from a local build: `nix-build-push [targets...]`
                  self.packages.${system}.nix-build-push
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
            NPM_PACKAGE = "${bombadil.npm-package}";
          };

          release = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.release ];
          };
        };
      }
    );
}
