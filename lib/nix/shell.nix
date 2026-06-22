{
  pkgs ?
    import
      (fetchTarball {
        url = "https://github.com/NixOS/nixpkgs/archive/4c1018dae018162ec878d42fec712642d214fdfa.tar.gz";
        sha256 = "sha256-ar3rofg+awPB8QXDaFJhJ2jJhu+KqN/PRCXeyuXR76E=";
      })
      {
        overlays = [
          (import (fetchTarball {
            url = "https://github.com/oxalica/rust-overlay/archive/4d6fee71fea68418a48992409b47f1183d0dd111.tar.gz";
            sha256 = "sha256-5TD8MYqLMcJi9yV/9jq2dVUPtnu/lKZPD61esQCgvqs=";
          }))
        ];
      },
}:
let
  rustToolchain = pkgs.rust-bin.stable.latest.default.override {
    targets = [ "wasm32-unknown-unknown" ];
  };

  # Pinned to match the `wasm-bindgen = "=X"` line in Cargo.toml.
  # Bump the two together — the CLI must match the runtime crate version
  # exactly. After a bump, replace the hash with the real one from the
  # nix-shell error. Uses the prebuilt GitHub release tarball to avoid the
  # from-source cargo vendor fetch (which currently 403s on crates.io).
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
pkgs.mkShell (
  {
    shellHook = ''
      unset TMPDIR TMP TEMP TEMPDIR
      export CC=${pkgs.clang}/bin/clang
      export CXX=${pkgs.clang}/bin/clang++
    '';
    CARGO_INSTALL_ROOT = "${toString ../../.}/.cargo";
    GHOSTTY_SOURCE_DIR = ghosttySrc;
    GHOSTTY_ZIG_SYSTEM_DIR = ghosttyZigDeps;

    nativeBuildInputs = [ rustToolchain ];

    packages = [ (pkgs.callPackage ./cargo-hotpath.nix { }) ];

    buildInputs =
      with pkgs;
      [
        # Rust dev tools
        rust-analyzer
        cargo-insta
        sccache

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
)
