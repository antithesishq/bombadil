{
  callPackage,
  lib,
  runCommand,
  stdenv,
  pkg-config,
  esbuild,
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
    src = ./..;
    filter =
      path: type:
      (lib.hasSuffix ".ts" path)
      || (lib.hasSuffix ".json" path)
      || (lib.hasSuffix ".snap" path)
      || (lib.hasSuffix ".html" path)
      || (lib.hasSuffix ".xml" path)
      || (lib.hasSuffix ".js" path)
      || (craneLib.filterCargoSources path type);
  };

  # Source with a normalized version so that Bombadil version bumps don't
  # invalidate the deps derivation hash. This is especially useful when
  # doing releases so that GitHub Actions doesn't have to rebuild deps that
  # haven't changed.
  depsSrc = runCommand "bombadil-deps-src" { } ''
    cp -r ${src} $out
    chmod -R +w $out
    sed -i '0,/^version = /{s/^version = .*/version = "0.0.0"/}' $out/Cargo.toml
    sed -i '/^name = "bombadil"/{n;s/^version = .*/version = "0.0.0"/}' $out/Cargo.lock
  '';

  commonArgs = {
    inherit src;
    nativeBuildInputs = [
      esbuild
    ];
  };
  depsArgs = commonArgs // {
    src = depsSrc;
    pname = "bombadil";
    version = "stable";
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

  types = callPackage ./types.nix { inherit src; };

  tests = craneLib.cargoTest (
    commonArgs
    // {
      inherit cargoArtifacts;
      nativeCheckInputs = [ chromium ];
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
      cargoClippyExtraArgs = "--all-targets -- -D warnings";
    }
  );

  fmt = craneLib.cargoFmt {
    inherit (commonArgs) src;
  };
}
