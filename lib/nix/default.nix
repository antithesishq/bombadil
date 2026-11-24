{
  callPackage,
  lib,
  runCommand,
  stdenv,
  pkg-config,
  trunk,
  wasm-bindgen-cli,
  binaryen,
  apple-sdk ? null,
  chromium,
  google-chrome,
  freefont_ttf,
  makeFontsConf,
  cacert,
  libiconv ? null,
  craneLib,
  craneLibStatic,
  libhegelC,
  darwin ? null,
  ghosttyLib,
  ghosttyLibStatic,
}:
let
  ghosttyBuildInputs = lib.optionals stdenv.isDarwin [
    # Put the SDK in buildInputs so Nix's Darwin SDK hook owns SDKROOT and
    # DEVELOPER_DIR. libiconv is propagated by newer SDKs, but keeping it here
    # preserves compatibility with older nixpkgs revisions.
    apple-sdk
    libiconv
  ];
  src = lib.cleanSourceWith {
    src = ../..;
    # Directories outside the nix build closure:
    # - `examples/` holds runtime-only specification files consumed by
    #   `bombadil terminal test --specification …`.
    # - `docs/` builds via its own derivation (`docs/manual/default.nix`);
    #   nothing in the workspace references it at build time.
    # - `.github/` is CI config and issue templates; never an input to the
    #   nix build.
    # Excluding them keeps edits there from invalidating bin/checks/tests.
    filter =
      path: type:
      !(lib.hasInfix "/examples/" path)
      && !(lib.hasInfix "/docs/" path)
      && !(lib.hasInfix "/.github/" path)
      && (
        (lib.hasSuffix ".ts" path)
        || (lib.hasSuffix ".json" path)
        || (lib.hasSuffix ".snap" path)
        || (lib.hasSuffix ".html" path)
        || (lib.hasSuffix ".xml" path)
        || (lib.hasSuffix ".js" path)
        || (lib.hasSuffix ".c" path)
        || (lib.hasSuffix ".css" path)
        || (lib.hasSuffix ".txt" path)
        || (lib.hasSuffix ".dat" path)
        || (craneLib.filterCargoSources path type)
      );
  };

  # Workspace crate names, extracted from each member's Cargo.toml.
  crateNames = lib.pipe (builtins.readDir ../../lib) [
    (lib.filterAttrs (_: type: type == "directory"))
    (
      dirs:
      lib.filter (name: builtins.pathExists (../../lib + "/${name}/Cargo.toml")) (builtins.attrNames dirs)
    )
    (map (dir: (builtins.fromTOML (builtins.readFile (../../lib + "/${dir}/Cargo.toml"))).package.name))
  ];

  # Minimal source for deps: only cargo metadata so that .ts/.html/etc.
  # changes don't invalidate the deps derivation hash. Versions are also
  # zeroed so that version bumps don't cause rebuilds.
  depsSrc =
    let
      cargoOnly = lib.cleanSourceWith {
        src = ../..;
        filter = path: type: craneLib.filterCargoSources path type;
      };
    in
    runCommand "source" { } ''
      cp -r ${cargoOnly} $out
      chmod -R +w $out
      sed -i '0,/^version = /{s/^version = .*/version = "0.0.0"/}' $out/Cargo.toml
      for crate in ${lib.concatStringsSep " " crateNames}; do
        sed -i "/^name = \"$crate\"/{n;s/^version = .*/version = \"0.0.0\"/}" $out/Cargo.lock
      done
    '';

  cargoVendorDir = craneLib.vendorCargoDeps { inherit src; };

  commonArgs = {
    inherit src cargoVendorDir;
    nativeBuildInputs = [
      trunk
      wasm-bindgen-cli
      binaryen
      pkg-config
    ];
    buildInputs = ghosttyBuildInputs;
    # GHOSTTY_SOURCE_DIR would override pkg-config and rebuild Ghostty in Cargo.
    PKG_CONFIG_PATH = "${ghosttyLib.dev}/lib/pkgconfig";
    # Exclude the inspect crate from workspace builds since it
    # targets wasm32 and is built by bombadil-cli's build script.
    cargoExtraArgs = "--workspace --exclude bombadil-inspect";
    HEGEL_C_LIB_DIR = "${libhegelC}/lib";
  };
  staticGhosttyArgs = {
    PKG_CONFIG_PATH = "${ghosttyLibStatic.dev}/lib/pkgconfig";
    # This archive matches the musl target, not the build host's libc.
    PKG_CONFIG_ALLOW_CROSS = "1";
  };
  depsArgs = commonArgs // {
    src = depsSrc;
    pname = "bombadil";
    version = "stable";
    nativeBuildInputs = [ pkg-config ];
    buildInputs = ghosttyBuildInputs;
  };
  cargoArtifacts = craneLib.buildDepsOnly depsArgs;
  cargoArtifactsStatic = craneLibStatic.buildDepsOnly (depsArgs // staticGhosttyArgs);

  testPreCheck = ''
    export FONTCONFIG_FILE=${makeFontsConf { fontDirectories = [ freefont_ttf ]; }}
    export SSL_CERT_FILE=${cacert}/etc/ssl/certs/ca-bundle.crt
    export HOME=$(mktemp -d)
    mkdir -p $HOME/.cache $HOME/.config $HOME/.local $HOME/.pki
    mkdir -p $HOME/.config/google-chrome/Crashpad
    export XDG_CONFIG_HOME=$HOME/.config
    export XDG_CACHE_HOME=$HOME/.cache
    export INSTA_WORKSPACE_ROOT=$(pwd)
    export INSTA_UPDATE=no
  '';

  buildBinary =
    { linkStatically }:
    ((if linkStatically then craneLibStatic else craneLib).buildPackage (
      commonArgs
      // {
        inherit cargoArtifacts;
        doCheck = false;
        pname = "bombadil";
        cargoExtraArgs = "-p bombadil-cli";
        meta = {
          mainProgram = "bombadil";
          description = ''
            Property-based testing for web UIs, autonomously exploring and validating
            correctness properties, finding harder bugs earlier.
          '';
        };
      }
      // lib.optionalAttrs linkStatically {
        cargoArtifacts = cargoArtifactsStatic;
      }
      // lib.optionalAttrs linkStatically staticGhosttyArgs
      // lib.optionalAttrs stdenv.isDarwin {
        # Rewrite Nix store dylib references to system paths so the buildBinary
        # is distributable outside of Nix.
        postFixup = ''
          for nixlib in $(otool -L $out/bin/bombadil | grep /nix/store | awk '{print $1}'); do
            base=$(basename "$nixlib")
            install_name_tool -change "$nixlib" "/usr/lib/$base" $out/bin/bombadil
          done
        '';
        nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ darwin.autoSignDarwinBinariesHook ];
      }
    ));
in

let
  bin = buildBinary { linkStatically = stdenv.isLinux; };
in
{
  inherit bin;

  tests-exit-codes =
    runCommand "bombadil-tests-exit-codes"
      {
        nativeBuildInputs = [ (if stdenv.isDarwin then google-chrome else chromium) ];
      }
      ''
        ${testPreCheck}
        bash ${./tests-exit-codes.sh} ${bin}/bin/bombadil
        touch $out
      '';

  npm-package = callPackage ./npm-package.nix { inherit src; };

  typescript-reference = craneLibStatic.buildPackage (
    commonArgs
    // staticGhosttyArgs
    // {
      src = depsSrc;
      doCheck = false;
      pname = "typescript-reference";
      cargoExtraArgs = "-p typescript-reference";
      cargoArtifacts = cargoArtifactsStatic;
    }
  );

  cli-reference = craneLib.buildPackage (
    commonArgs
    // {
      inherit cargoArtifacts;
      doCheck = false;
      pname = "cli-reference";
      cargoExtraArgs = "-p cli-reference";
    }
  );

  tests-unit = craneLib.cargoTest (
    commonArgs
    // {
      inherit cargoArtifacts;
      pname = "bombadil-tests-unit";
      cargoExtraArgs = "--workspace --exclude bombadil-inspect --exclude bombadil-browser-integration-tests";
      preCheck = testPreCheck;
    }
  );

  tests-browser = craneLib.cargoTest (
    commonArgs
    // {
      inherit cargoArtifacts;
      nativeCheckInputs = [ (if stdenv.isDarwin then google-chrome else chromium) ];
      pname = "bombadil-tests-browser";
      cargoExtraArgs = "-p bombadil-browser-integration-tests";
      cargoTestExtraArgs = lib.optionalString stdenv.isDarwin "-- --test-threads=1";
      preCheck = testPreCheck;
    }
  );

  clippy = craneLib.cargoClippy (
    commonArgs
    // {
      inherit cargoArtifacts;
      pname = "bombadil";
      cargoClippyExtraArgs = "--all-targets -- -D warnings";
    }
  );

  fmt = craneLib.cargoFmt {
    inherit (commonArgs) src;
    pname = "bombadil";
  };
}
// lib.optionalAttrs stdenv.isLinux {
  bin-dynamic = buildBinary { linkStatically = false; };
}
# chrome-headless-shell.nix only packages the linux64 build.
// lib.optionalAttrs (stdenv.hostPlatform.system == "x86_64-linux") {
  # Begin frame control only works in chrome-headless-shell, so its code
  # paths need their own run of the browser tests.
  tests-browser-headless-shell = craneLib.cargoTest (
    commonArgs
    // {
      inherit cargoArtifacts;
      # Tests marked `requires_full_browser` still run with chromium.
      nativeCheckInputs = [
        chromium
        (callPackage ./chrome-headless-shell.nix { })
      ];
      # Keep this name short. Chromium puts a Unix socket under the build
      # directory, which is named after the derivation on CI, and aborts
      # when its path is longer than 107 bytes.
      pname = "bombadil-tests-headless";
      cargoExtraArgs = "-p bombadil-browser-integration-tests";
      preCheck = testPreCheck + ''
        export BOMBADIL_TEST_CHROME_VARIANT=headless-shell
      '';
    }
  );
}
