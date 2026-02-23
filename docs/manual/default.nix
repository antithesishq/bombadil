{
  lib,
  stdenvNoCC,
  pandoc,
  texlive,
  ibm-plex,
  gnumake,
  esbuild,
}:
let
  version =
    (builtins.fromTOML (builtins.readFile ../../Cargo.toml)).package.version;

  texliveBundle = texlive.combine {
    inherit (texlive)
      scheme-basic
      lualatex-math
      luatexbase
      fontspec
      unicode-math
      amsmath
      tools
      sectsty
      xcolor
      hyperref
      geometry
      fancyvrb
      booktabs
      caption
      fancyhdr
      titling
      parskip
      listings
      lm
      tcolorbox
      pgf
      environ
      etoolbox
      mdwtools
      fontawesome5
      ;
  };
in
stdenvNoCC.mkDerivation {
  pname = "bombadil-manual";
  inherit version;

  src = lib.cleanSourceWith {
    src = ./.;
    filter =
      path: type:
      (lib.hasSuffix ".md" path)
      || (lib.hasSuffix ".yaml" path)
      || (lib.hasSuffix ".html" path)
      || (lib.hasSuffix ".css" path)
      || (lib.hasSuffix ".js" path)
      || (lib.hasSuffix ".lua" path)
      || (baseNameOf path == "Makefile")
      || (type == "directory");
  };

  nativeBuildInputs = [
    pandoc
    texliveBundle
    gnumake
    esbuild
  ];

  OSFONTDIR = "${ibm-plex}/share/fonts/opentype";

  buildPhase = ''
    runHook preBuild
    export HOME=$(mktemp -d)
    make all VERSION=${version}
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p $out
    cp -r target/html/* $out/
    rm -f $out/.built
    cp target/bombadil-manual.pdf $out/
    cp target/bombadil-manual.epub $out/
    cp target/bombadil-manual.txt $out/
    cp target/bombadil.1 $out/
    runHook postInstall
  '';
}
