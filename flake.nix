{
  description = "Property-based testing for web UIs";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ];
        };

        # Pinned to match the `wasm-bindgen = "=X"` line in Cargo.toml.
        # Bump the two together — the CLI must match the runtime crate version
        # exactly. After a bump, run `nix develop` and replace each
        # `lib.fakeHash` with the real hash from the error message.
        # Uses the prebuilt GitHub release tarball to avoid the from-source
        # cargo vendor fetch (which currently 403s on crates.io).
        wasmBindgenCli =
          let
            version = "0.2.125";
            asset =
              if pkgs.stdenv.isLinux && pkgs.stdenv.isx86_64 then
                "x86_64-unknown-linux-musl"
              else if pkgs.stdenv.isLinux && pkgs.stdenv.isAarch64 then
                "aarch64-unknown-linux-gnu"
              else if pkgs.stdenv.isDarwin && pkgs.stdenv.isAarch64 then
                "aarch64-apple-darwin"
              else if pkgs.stdenv.isDarwin && pkgs.stdenv.isx86_64 then
                "x86_64-apple-darwin"
              else
                throw "wasm-bindgen-cli: unsupported platform";
          in
          pkgs.stdenv.mkDerivation {
            pname = "wasm-bindgen-cli";
            inherit version;
            src = pkgs.fetchurl {
              url = "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${version}/wasm-bindgen-${version}-${asset}.tar.gz";
              hash = "sha256-Idge90FKClhYYaYOpK4reXDsyu0J1KTgX4vEsVmCfeo=";
            };
            nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.autoPatchelfHook
            ];
            installPhase = ''
              runHook preInstall
              mkdir -p $out/bin
              install -m755 wasm-bindgen wasm-bindgen-test-runner wasm2es6js $out/bin/
              runHook postInstall
            '';
            meta = {
              description = "CLI for wasm-bindgen, pinned to match Cargo.toml's runtime crate";
              mainProgram = "wasm-bindgen";
            };
          };

        # Pinned to match `GHOSTTY_COMMIT` in libghostty-vt-sys's build.rs at
        # the `libghostty-vt` rev used by `lib/bombadil-terminal/Cargo.toml`.
        # Bump these together when updating libghostty-vt. libghostty-vt-sys's
        # build.rs reads GHOSTTY_SOURCE_DIR / GHOSTTY_ZIG_SYSTEM_DIR to skip
        # its in-tree clone and use this pre-fetched source + zig deps.
        ghosttySrc = pkgs.fetchFromGitHub {
          owner = "ghostty-org";
          repo = "ghostty";
          rev = "bfe633a9487892ff3d27ed727db540267f22ef90";
          sha256 = "1zmybfhrz64h6kibx23ixqsi7x9aw7c3szyb39zswh7mvg517297";
        };
        ghosttyZigDeps = pkgs.callPackage "${ghosttySrc}/build.zig.zon.nix" {
          name = "bombadil-ghostty-zig-deps";
        };
      in
      {
        devShells = {
          default = pkgs.mkShell (
            {
              shellHook = ''
                export CC=${pkgs.clang}/bin/clang
                export CXX=${pkgs.clang}/bin/clang++
              '';
              CARGO_INSTALL_ROOT = "${toString ./.}/.cargo";
              GHOSTTY_SOURCE_DIR = ghosttySrc;
              GHOSTTY_ZIG_SYSTEM_DIR = ghosttyZigDeps;

              nativeBuildInputs = [ rustToolchain ];

              packages = [ (pkgs.callPackage ./nix/cargo-hotpath.nix { }) ];

              buildInputs =
                with pkgs;
                [
                  # Rust dev tools
                  rust-analyzer
                  cargo-insta

                  # Nix tooling
                  nil

                  # Native build deps for bombadil-terminal (libghostty-vt-sys)
                  zig_0_15
                  pkg-config
                  git
                  cmake
                  clang

                  # WASM / Inspect UI
                  trunk
                  wasmBindgenCli
                  binaryen

                  # TS/JS
                  typescript
                  typescript-language-server
                  bun
                  biome

                  # Release scripts (lib/release/*.py)
                  python3
                  gh
                  basedpyright
                  black
                ]
                ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
                  chromium
                ]
                ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
                  libiconv
                  cctools
                  xcbuild
                ];
            }
            // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
              # override how chromiumoxide finds the chromium executable
              CHROME = pkgs.lib.getExe pkgs.chromium;
            }
          );

          manual = pkgs.mkShell {
            OSFONTDIR = "${pkgs.ibm-plex}/share/fonts/opentype";
            buildInputs = with pkgs; [
              pandoc
              gnumake
              esbuild
              watchexec
              browser-sync
              concurrently
              (texlive.combine {
                inherit (texlive)
                  scheme-basic
                  lualatex-math
                  luatexbase
                  fontspec
                  unicode-math
                  amsmath
                  tools
                  sectsty
                  xcolor
                  hyperref
                  geometry
                  fancyvrb
                  booktabs
                  caption
                  fancyhdr
                  titling
                  parskip
                  listings
                  lm
                  tcolorbox
                  pgf
                  environ
                  etoolbox
                  mdwtools
                  fontawesome5
                  ;
              })
            ];
          };
        };
      }
    );
}
