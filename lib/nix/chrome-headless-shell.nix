{
  alsa-lib,
  at-spi2-atk,
  at-spi2-core,
  atk,
  autoPatchelfHook,
  dbus,
  expat,
  fetchurl,
  glib,
  libgbm,
  libxkbcommon,
  nspr,
  nss,
  stdenv,
  udev,
  unzip,
  xorg,
}:
stdenv.mkDerivation rec {
  pname = "chrome-headless-shell";
  version = "157.0.8090.0";

  src = fetchurl {
    url = "https://storage.googleapis.com/chrome-for-testing-public/${version}/linux64/chrome-headless-shell-linux64.zip";
    sha256 = "sha256-2d32B6KOteFqJv1C2S2G4FZg8ZEydyIVN+PMW8EoumE=";
  };

  nativeBuildInputs = [
    unzip
    autoPatchelfHook
  ];

  # autoPatchelfHook fails the build if any NEEDED library is missing here.
  buildInputs = [
    alsa-lib
    at-spi2-atk
    at-spi2-core
    atk
    dbus
    expat
    glib
    libgbm
    libxkbcommon
    nspr
    nss
    stdenv.cc.cc.lib
    udev
    xorg.libX11
    xorg.libXcomposite
    xorg.libXdamage
    xorg.libXext
    xorg.libXfixes
    xorg.libXrandr
    xorg.libxcb
  ];

  unpackPhase = ''
    unzip $src
  '';

  installPhase = ''
    mkdir -p $out/bin
    cp -r chrome-headless-shell-linux64/* $out/bin/

    # Bombadil looks explicitly for "chrome" or "chromium" or honors PATH fallback.
    # Symlink it so it's transparently matched if a target path overrides it.
    ln -s $out/bin/chrome-headless-shell $out/bin/chrome
  '';
}
