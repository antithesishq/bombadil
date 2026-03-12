{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    gitignore.url = "github:hercules-ci/gitignore.nix";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      gitignore,
    }:
    let
      inherit (gitignore.lib) gitignoreSource;
    in
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = (
          import nixpkgs {
            inherit system;
            overlays = [ ];
          }
        );
      in
      {
        devShells = {
          default = pkgs.mkShell {
            inputsFrom = [ ];
            buildInputs = with pkgs; [
              graphviz
              python3
              python3Packages.pillow
            ];
          };
        };
      }
    );
}
