{
  dockerTools,
  callPackage,
  buildEnv,
  coreutils,
  runtimeShell,
  bash,
  chromium,
  bombadil,
}:
dockerTools.buildImage {
  name = "bombadil_docker";
  copyToRoot = buildEnv {
    name = "image_root";
    paths = [
      bombadil
      coreutils
      bash
      chromium
    ];
    pathsToLink = [ "/bin" ];
  };
  runAsRoot = ''
    #!${runtimeShell}
    ${dockerTools.shadowSetup}
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
    Cmd = [ ];
    Entrypoint = [
      "${bombadil}/bin/bombadil"
      "test"
      "--headless"
    ];
  };
}
