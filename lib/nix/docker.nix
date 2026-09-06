{
  dockerTools,
  buildEnv,
  coreutils,
  runtimeShell,
  bashInteractive,
  chromium,
  curl,
  bombadil,
  # Fonts
  fontconfig,
  makeFontsConf,
  liberation_ttf,
  noto-fonts,
  noto-fonts-color-emoji,
}:
let
  version = (builtins.fromTOML (builtins.readFile ../../Cargo.toml)).workspace.package.version;
  fontConfig = makeFontsConf {
    fontDirectories = [
      liberation_ttf
      noto-fonts
      noto-fonts-color-emoji
    ];
  };
in
dockerTools.buildLayeredImage {
  name = "antithesishq/bombadil";
  tag = version;
  contents = [
    bombadil
    coreutils
    bashInteractive
    fontconfig
    liberation_ttf
    noto-fonts
    noto-fonts-color-emoji
    chromium
    curl
  ];
  enableFakechroot = true;
  fakeRootCommands = ''
    #!${runtimeShell}
    ${dockerTools.shadowSetup}

    mkdir -p /usr/bin
    ln -s /bin/env /usr/bin/env

    useradd -r browser

    mkdir -p tmp
    chmod 1777 tmp

    mkdir -p /home/browser/.cache /home/browser/.config /home/browser/.local /home/browser/.pki
    chown -R browser /home/browser

    # https://github.com/chrome-php/chrome/issues/649
    mkdir -p /var/www/.config/google-chrome/Crashpad
    chown -R browser /var/www/.config
  '';
  config = {
    User = "browser";
    Entrypoint = [
      "${bombadil}/bin/bombadil"
    ];
    Cmd = [
      "browser"
      "test"
      "--headless"
      "--no-sandbox"
    ];
    Env = [
      "FONTCONFIG_FILE=${fontConfig}"
    ];
  };
}
