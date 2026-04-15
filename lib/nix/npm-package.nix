{
  lib,
  runCommand,
  typescript,
  writeText,
  src,
}:
let
  version = (builtins.fromTOML (builtins.readFile ../../Cargo.toml)).workspace.package.version;

  packageJson = writeText "package.json" (
    builtins.toJSON {
      name = "@antithesishq/bombadil";
      inherit version;
      description = "Property-based testing tool for web UIs";
      types = "./dist/index.d.ts";
      bin = {
        bombadil = "./bin/bombadil.js";
      };
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
        "bin"
        "binaries"
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

    [Bombadil](https://github.com/antithesishq/bombadil) is property-based testing
    for web UIs, autonomously exploring and validating correctness properties,
    *finding harder bugs earlier*.

    ## Install

    ```
    npm install --save-dev @antithesishq/bombadil
    ```

    ## Usage

    Add a script to your `package.json`:

    ```json
    {
      "scripts": {
        "test": "bombadil test https://your-app.example.com"
      }
    }
    ```

    Then run it with `npm test`. This also gives you TypeScript type definitions
    for writing specifications.

    Write custom properties:

    ```typescript
    import { always, eventually, extract } from "@antithesishq/bombadil";

    const title = extract((state) =>
      state.document.querySelector("h1")?.textContent ?? ""
    );

    export const has_title = always(() => title.current.trim() !== "");
    ```

    Or re-export the default properties:

    ```typescript
    export * from "@antithesishq/bombadil/defaults";
    ```

    ## Documentation

    See the [Bombadil repository](https://github.com/antithesishq/bombadil) for
    full usage instructions and more examples.
  '';

  # Wrapper script that resolves the platform-specific binary at runtime.
  # Backtick template literals use ''${} to escape Nix interpolation.
  wrapperScript = writeText "bombadil.js" ''
    #!/usr/bin/env node
    "use strict";

    const path = require("path");
    const { spawnSync } = require("child_process");
    const os = require("os");

    const binaries = {
      "linux-x64":    "bombadil-linux-x64",
      "linux-arm64":  "bombadil-linux-arm64",
      "darwin-arm64": "bombadil-darwin-arm64",
    };

    const key = `''${os.platform()}-''${os.arch()}`;
    const binary = binaries[key];

    if (!binary) {
      process.stderr.write(`bombadil: unsupported platform ''${key}\n`);
      process.exit(1);
    }

    const result = spawnSync(
      path.join(__dirname, "..", "binaries", binary),
      process.argv.slice(2),
      { stdio: "inherit" }
    );

    if (result.error) {
      process.stderr.write(`bombadil: ''${result.error.message}\n`);
      process.exit(1);
    }

    process.exit(result.status ?? 1);
  '';
in
runCommand "bombadil-npm-package-${version}"
  {
    nativeBuildInputs = [ typescript ];
  }
  ''
    mkdir -p $out/dist $out/bin

    tsc \
      -p ${src}/lib/bombadil/src/specification/tsconfig.json \
      --target es6 \
      --declaration \
      --emitDeclarationOnly \
      --stripInternal \
      --outDir $out/dist

    cp ${packageJson} $out/package.json
    cp ${readme} $out/README.md
    cp ${wrapperScript} $out/bin/bombadil.js
    chmod +x $out/bin/bombadil.js
  ''
