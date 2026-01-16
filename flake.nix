{
  description = "Browser testing on Antithesis";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
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
        packages = {
          default = pkgs.callPackage ./nix/executable.nix { };
          docker = pkgs.callPackage ./nix/docker.nix { };
        };

        devShells = {
          default = pkgs.mkShell {
            CARGO_INSTALL_ROOT = "${toString ./.}/.cargo";
            inputsFrom = [ self.packages.${system}.default ];
            buildInputs = with pkgs; [
              rust-analyzer
              rustfmt
              crate2nix
              cargo-insta
              chromium
              typescript
              typescript-language-server
              nil
              esbuild
            ];
          };
        };
      }
    );
}
