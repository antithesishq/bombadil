{
  callPackage,
  lib,
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
      version = "0.1.0";
      meta = {
        mainProgram = "bombadil";
        description = ''
          Property-based testing for web UIs, autonomously exploring and validating
          correctness properties, finding harder bugs earlier.
        '';
      };
    }
  );

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
