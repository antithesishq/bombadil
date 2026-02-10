{
  callPackage,
  lib,
  stdenv,
  pkg-config,
  esbuild,
  chromium,
  craneLib,
  craneLibStatic,
}:
let
  src = lib.cleanSourceWith {
    src = ./..;
    filter =
      path: type:
      (lib.hasSuffix ".ts" path)
      || (lib.hasSuffix ".json" path)
      || (lib.hasSuffix ".snap" path)
      || (lib.hasSuffix ".html" path)
      || (lib.hasSuffix ".js" path)
      || (craneLib.filterCargoSources path type);
  };

  commonArgs = {
    inherit src;
    nativeBuildInputs = [
      esbuild
    ];
  };
  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
  cargoArtifactsStatic = craneLibStatic.buildDepsOnly commonArgs;
in
{
  static = craneLib.buildPackage (
    commonArgs
    // {
      inherit cargoArtifacts;
      doCheck = false;
      pname = "bombadil";
      meta = {
        mainProgram = "bombadil";
        description = ''
          Property-based testing for web UIs, autonomously exploring and validating
          correctness properties, finding harder bugs earlier.
        '';
      };
    }
    // lib.optionalAttrs stdenv.isLinux {
      CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
      CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
    }
  );

  types = callPackage ./types.nix { inherit src; };

  tests = craneLib.cargoTest (
    commonArgs
    // {
      inherit cargoArtifacts;
      nativeCheckInputs = [ chromium ];
      preCheck = ''
        export HOME=$(mktemp -d)
          mkdir -p $HOME/.cache $HOME/.config $HOME/.local $HOME/.pki
          mkdir -p $HOME/.config/google-chrome/Crashpad
          export XDG_CONFIG_HOME=$HOME/.config
          export XDG_CACHE_HOME=$HOME/.cache
          export INSTA_WORKSPACE_ROOT=$(pwd)
          export INSTA_UPDATE=no
      '';
    }
  );

  clippy = craneLib.cargoClippy (
    commonArgs
    // {
      inherit cargoArtifacts;
      cargoClippyExtraArgs = "--all-targets -- -D warnings";
    }
  );

  fmt = craneLib.cargoFmt {
    inherit (commonArgs) src;
  };
}
