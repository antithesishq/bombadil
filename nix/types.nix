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
      description = "Type definitions for writing Bombadil specifications";
      types = "./dist/index.d.ts";
      exports = {
        "." = {
          types = "./dist/index.d.ts";
        };
        "./defaults" = {
          types = "./dist/defaults.d.ts";
        };
        "./defaults/actions" = {
          types = "./dist/defaults/actions.d.ts";
        };
        "./defaults/properties" = {
          types = "./dist/defaults/properties.d.ts";
        };
        "./random" = {
          types = "./dist/random.d.ts";
        };
        "./actions" = {
          types = "./dist/actions.d.ts";
        };
        "./internal" = {
          types = "./dist/internal.d.ts";
        };
      };
      files = [
        "dist"
        "README.md"
      ];
      keywords = [
        "testing"
        "property-based-testing"
        "fuzzing"
        "web"
        "browser"
        "ui"
        "antithesis"
      ];
      license = "MIT";
      repository = {
        type = "git";
        url = "https://github.com/antithesishq/bombadil";
      };
      homepage = "https://github.com/antithesishq/bombadil";
    }
  );

  readme = writeText "README.md" ''
    # @antithesishq/bombadil

    [![Version](https://img.shields.io/badge/version-${version}-blue)](https://github.com/antithesishq/bombadil/releases/tag/v${version})

    Type definitions for writing [Bombadil](https://github.com/antithesishq/bombadil) specifications.

    Bombadil is property-based testing for web UIs, autonomously exploring and
    validating correctness properties, *finding harder bugs earlier*.

    ## Install

    ```
    npm install --save-dev @antithesishq/bombadil
    ```

    ## Usage

    Re-export the default properties:

    ```typescript
    export * from "@antithesishq/bombadil/defaults";
    ```

    Or write custom properties:

    ```typescript
    import { always, eventually, extract, now } from "@antithesishq/bombadil";

    const title = extract((state) =>
      state.document.querySelector("h1")?.textContent ?? ""
    );

    export const has_title = always(() => title.current.trim() !== "");
    ```

    ## Documentation

    See the [Bombadil repository](https://github.com/antithesishq/bombadil) for
    full usage instructions and more examples.
  '';
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
    cp ${readme} $out/README.md
  ''
