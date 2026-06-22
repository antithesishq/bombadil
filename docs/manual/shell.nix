{
  pkgs ? import (fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/4c1018dae018162ec878d42fec712642d214fdfa.tar.gz";
    sha256 = "sha256-ar3rofg+awPB8QXDaFJhJ2jJhu+KqN/PRCXeyuXR76E=";
  }) { },
}:
pkgs.mkShell {
  OSFONTDIR = "${pkgs.ibm-plex}/share/fonts/opentype";
  buildInputs = with pkgs; [
    pandoc
    gnumake
    esbuild
    watchexec
    browser-sync
    concurrently
    (texlive.combine {
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
    })
  ];
}
