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
        packages = {
          default = pkgs.callPackage ./. { inherit gitignoreSource; };
        };

        devShells = {
          default = pkgs.mkShell {
            CARGO_INSTALL_ROOT = "${toString ./.}/.cargo";
            TODOMVC = pkgs.fetchzip {
              url = "https://github.com/tastejs/todomvc/archive/refs/heads/master.zip";
              hash = "sha256-YlI6qx8Bm6atTJzYlQxp0qGpXJkoUxN+FnHyX0ALLgw=";
            };
            inputsFrom = [ self.packages.${system}.default ];
            buildInputs = with pkgs; [
              rust-analyzer
              rustfmt
              chromium
              typescript
              typescript-language-server
              esbuild
            ];
          };
        };
      }
    );
}
