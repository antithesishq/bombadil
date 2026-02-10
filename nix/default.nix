{
  callPackage,
  lib,
  stdenv,
  pkg-config,
  esbuild,
  typescript,
  chromium,
  craneLib,
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
      typescript
    ];
  };
  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
in
{
  bin = craneLib.buildPackage (
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
        export XDG_CONFIG_HOME=$(mktemp -d)
        export XDG_CACHE_HOME=$(mktemp -d)
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
