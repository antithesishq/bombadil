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
  cctools ? null,
  chromium,
  freefont_ttf,
  makeFontsConf,
  libiconv ? null,
  craneLib,
  craneLibStatic,
  darwin ? null,
  xcbuild ? null,
  # Ghostty source pinned to the commit referenced by libghostty-vt-sys's
  # build.rs (`GHOSTTY_COMMIT`). Provided as a vendored source tree so that
  # the Cargo build script can skip its in-tree `git clone` step (which has
  # no network access in the Nix sandbox).
  ghosttySrc,
  zig_0_15,
  git,
}:
let
  # Pre-fetched Zig package cache for ghostty's build.zig. Passed to zig via
  # `--system` (set up by libghostty-vt-sys's build.rs when
  # `GHOSTTY_ZIG_SYSTEM_DIR` is set), keeping the Zig build hermetic.
  ghosttyZigDeps = callPackage "${ghosttySrc}/build.zig.zon.nix" {
    name = "bombadil-ghostty-zig-deps";
  };
  ghosttyEnv = {
    GHOSTTY_SOURCE_DIR = "${ghosttySrc}";
    GHOSTTY_ZIG_SYSTEM_DIR = "${ghosttyZigDeps}";
  };
  ghosttyNativeBuildInputs = [
    zig_0_15
    pkg-config
    git
  ]
  ++ lib.optionals stdenv.isDarwin [
    # Ghostty's Zig build asks Zig to discover the native Darwin SDK via
    # xcode-select and xcrun. Nix's apple-sdk propagates xcrun, but xcbuild
    # provides xcode-select so Zig's findNative() path does not fail with
    # DarwinSdkNotFound in sandboxed CI builds.
    cctools
    xcbuild
  ];
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

  # hegeltest-c's build.rs only exists to verify that the checked-in
  # include/hegel.h matches what cbindgen would regenerate from src/lib.rs
  # — a drift check for upstream's own CI, not something we consume. It
  # runs `cargo metadata` against the vendored manifest, which fails when
  # any transitive (dashu-int, cbindgen, ...) has been bumped between
  # hegeltest's publish and our `cargo update`. The build.rs already has
  # an escape hatch for `cargo package`'s isolated target dir; vendored
  # copies hit the same class of problem from a path that hatch doesn't
  # match. Stub the build.rs so cbindgen never runs — we ship Rust
  # bindings, the header is for C/C++ consumers we don't have.
  cargoVendorDir = craneLib.vendorCargoDeps {
    inherit src;
    overrideVendorCargoPackage =
      pkg: drv:
      if lib.hasPrefix "hegeltest" pkg.name then
        drv.overrideAttrs (old: {
          postInstall = (old.postInstall or "") + ''
            # Only overwrite if the crate actually ships a build.rs.
            # hegeltest-c is the one we need to neutralize; siblings
            # don't have one and we don't want to add it.
            if [ -f $out/build.rs ]; then
              echo 'fn main() {}' > $out/build.rs
            fi
          '';
        })
      else
        drv;
  };

  commonArgs = {
    inherit src cargoVendorDir;
    nativeBuildInputs = [
      trunk
      wasm-bindgen-cli
      binaryen
    ]
    ++ ghosttyNativeBuildInputs;
    buildInputs = ghosttyBuildInputs;
    # Exclude the inspect crate from workspace builds since it
    # targets wasm32 and is built by bombadil-cli's build script.
    cargoExtraArgs = "--workspace --exclude bombadil-inspect";
  }
  // ghosttyEnv;
  depsArgs = commonArgs // {
    src = depsSrc;
    pname = "bombadil";
    version = "stable";
    nativeBuildInputs = ghosttyNativeBuildInputs;
    buildInputs = ghosttyBuildInputs;
  };
  cargoArtifacts = craneLib.buildDepsOnly depsArgs;
  cargoArtifactsStatic = craneLibStatic.buildDepsOnly depsArgs;

  testPreCheck = ''
    export FONTCONFIG_FILE=${makeFontsConf { fontDirectories = [ freefont_ttf ]; }}
    export HOME=$(mktemp -d)
    mkdir -p $HOME/.cache $HOME/.config $HOME/.local $HOME/.pki
    mkdir -p $HOME/.config/google-chrome/Crashpad
    export XDG_CONFIG_HOME=$HOME/.config
    export XDG_CACHE_HOME=$HOME/.cache
    export INSTA_WORKSPACE_ROOT=$(pwd)
    export INSTA_UPDATE=no
  '';
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
      nativeCheckInputs = [ chromium ];
      pname = "bombadil-tests-browser";
      cargoExtraArgs = "-p bombadil-browser-integration-tests";
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
