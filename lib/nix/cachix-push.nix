{
  writeShellApplication,
  cachix,
  nix,
}:
writeShellApplication {
  name = "nix-build-push";
  runtimeInputs = [
    cachix
    nix
  ];
  text = ''
    if [ $# -eq 0 ]; then
      set -- .#default
    fi
    exec cachix watch-exec bombadil -- nix build --no-link --print-build-logs "$@"
  '';
}
