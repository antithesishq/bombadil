{
  callPackage,
  pkg-config,
  esbuild,
  typescript,
  chromium,
}:
let
  customBuildRustCrateForPkgs =
    pkgs:
    pkgs.buildRustCrate.override {
      defaultCrateOverrides = pkgs.defaultCrateOverrides // {
        bombadil = attrs: {
          nativeBuildInputs = [
            esbuild
            typescript
          ];
        };
      };
    };
in
(callPackage ./Cargo.nix {
  buildRustCrateForPkgs = customBuildRustCrateForPkgs;
}).rootCrate.build.override
  {
    runTests = true;
    testInputs = [ chromium ];
    testPreRun = ''
      export XDG_CONFIG_HOME=$(mktemp -d)
      export XDG_CACHE_HOME=$(mktemp -d)

      export INSTA_WORKSPACE_ROOT=$CARGO_MANIFEST_DIR
      export INSTA_UPDATE=no
    '';
  }
