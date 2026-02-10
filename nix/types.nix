{
  lib,
  runCommand,
  typescript,
  writeText,
  src,
}:
let
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).package.version;

  packageJson = writeText "package.json" (
    builtins.toJSON {
      name = "@antithesishq/bombadil";
      inherit version;
      description = "Type definitions for Bombadil specifications";
      types = "./dist/index.d.ts";
      exports = {
        "." = {
          types = "./dist/index.d.ts";
        };
        "./defaults" = {
          types = "./dist/defaults.d.ts";
        };
        "./internal" = {
          types = "./dist/internal.d.ts";
        };
      };
      files = [ "dist" ];
      license = "MIT";
    }
  );
in
runCommand "bombadil-types-${version}"
  {
    nativeBuildInputs = [ typescript ];
  }
  ''
    mkdir -p $out/dist

    tsc \
      -p ${src}/src/specification/tsconfig.json \
      --target es6 \
      --declaration \
      --emitDeclarationOnly \
      --stripInternal \
      --outDir $out/dist

    cp ${packageJson} $out/package.json
  ''
