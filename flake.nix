{
  description = "Browser testing on Antithesis";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  nixConfig = {
    extra-substituters = "https://bombadil.cachix.org";
    extra-trusted-public-keys = "bombadil.cachix.org-1:6L4epM9zwhEcAwouNgBa8ENtsgLNfedtQgqtdnQhZiM=";
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

        apps = {
          default = {
            type = "app";
            program = "${self.packages.${system}.default}/bin/bombadil";
          };
        };

        devShells = {
          default = pkgs.mkShell {
            CARGO_INSTALL_ROOT = "${toString ./.}/.cargo";
            inputsFrom = [ self.packages.${system}.default ];
            buildInputs = with pkgs; [
              # Rust
              cargo
              rustc
              rust-analyzer
              rustfmt
              crate2nix
              cargo-insta

              # Nix
              nil

              # TS/JS
              typescript
              typescript-language-server
              esbuild
              bun
              biome

              # Runtime
              chromium
            ];
          };
        };
      }
    );
}
