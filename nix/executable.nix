{ callPackage, rustPlatform, pkg-config, esbuild, chromium, }:
let
  customBuildRustCrateForPkgs = pkgs:
    pkgs.buildRustCrate.override {
      defaultCrateOverrides = pkgs.defaultCrateOverrides // {
        bombadil = attrs: { nativeBuildInputs = [ esbuild ]; };
      };
    };
in (callPackage ./Cargo.nix {
  buildRustCrateForPkgs = customBuildRustCrateForPkgs;
}).rootCrate.build
