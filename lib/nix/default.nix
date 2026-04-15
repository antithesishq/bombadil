{
  callPackage,
  lib,
  runCommand,
  stdenv,
  pkg-config,
  esbuild,
  trunk,
  wasm-bindgen-cli,
  binaryen,
  chromium,
  freefont_ttf,
  makeFontsConf,
  craneLib,
  craneLibStatic,
  cargoTarget ? "x86_64-unknown-linux-musl",
  darwin ? null,
}:
let
  src = lib.cleanSourceWith {
    src = ../..;
    filter =
      path: type:
      (lib.hasSuffix ".ts" path)
      || (lib.hasSuffix ".json" path)
      || (lib.hasSuffix ".snap" path)
      || (lib.hasSuffix ".html" path)
      || (lib.hasSuffix ".xml" path)
      || (lib.hasSuffix ".js" path)
      || (lib.hasSuffix ".css" path)
      || (lib.hasSuffix ".txt" path)
      || (lib.hasSuffix ".dat" path)
      || (craneLib.filterCargoSources path type);
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
    runCommand "bombadil-deps-src" { } ''
      cp -r ${cargoOnly} $out
      chmod -R +w $out
      sed -i '0,/^version = /{s/^version = .*/version = "0.0.0"/}' $out/Cargo.toml
      for crate in ${lib.concatStringsSep " " crateNames}; do
        sed -i "/^name = \"$crate\"/{n;s/^version = .*/version = \"0.0.0\"/}" $out/Cargo.lock
      done
    '';

  commonArgs = {
    inherit src;
    nativeBuildInputs = [
      esbuild
      trunk
      wasm-bindgen-cli
      binaryen
    ];
    # Exclude the inspect crate from workspace builds since it
    # targets wasm32 and is built by bombadil-cli's build script.
    cargoExtraArgs = "--workspace --exclude bombadil-inspect";
  };
  depsArgs = commonArgs // {
    src = depsSrc;
    pname = "bombadil";
    version = "stable";
    nativeBuildInputs = [ ];
  };
  cargoArtifacts = craneLib.buildDepsOnly depsArgs;
  cargoArtifactsStatic = craneLibStatic.buildDepsOnly depsArgs;
in
{
  bin = (if stdenv.isLinux then craneLibStatic else craneLib).buildPackage (
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
    // lib.optionalAttrs stdenv.isLinux {
      cargoArtifacts = cargoArtifactsStatic;
      CARGO_BUILD_TARGET = cargoTarget;
      CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
    }
    // lib.optionalAttrs stdenv.isDarwin {
      # Rewrite Nix store dylib references to system paths so the binary
      # is distributable outside of Nix.
      postFixup = ''
        for nixlib in $(otool -L $out/bin/bombadil | grep /nix/store | awk '{print $1}'); do
          base=$(basename "$nixlib")
          install_name_tool -change "$nixlib" "/usr/lib/$base" $out/bin/bombadil
        done
      '';
      nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ darwin.autoSignDarwinBinariesHook ];
    }
  );

  npm-package = callPackage ./npm-package.nix { inherit src; };

  tests = craneLib.cargoTest (
    commonArgs
    // {
      inherit cargoArtifacts;
      nativeCheckInputs = [ chromium ];
      pname = "bombadil";
      preCheck = ''
        export FONTCONFIG_FILE=${makeFontsConf { fontDirectories = [ freefont_ttf ]; }}
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
      pname = "bombadil";
      cargoClippyExtraArgs = "--all-targets -- -D warnings";
    }
  );

  fmt = craneLib.cargoFmt {
    inherit (commonArgs) src;
    pname = "bombadil";
  };
}
