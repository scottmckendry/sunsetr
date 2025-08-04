{
  description = "Automatic blue light filter for Hyprland, Niri, and everything Wayland";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
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
        pkgs = import nixpkgs { inherit system; };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "sunsetr";
          version = "0.6.4";
          src = pkgs.fetchFromGitHub {
            # owner = "psi4j";
            owner = "scottmckendry"; # TODO: update owner to psi4j
            repo = "sunsetr";
            rev = "v0.6.4";
            sha256 = "F1tShEgaEQpU0niJ7U/vHsEMTwrcmROgo/3TbgkQiEY="; # TODO: dynamically update this on release, along with the tagged version
          };
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
        };
        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/sunsetr";
        };
      }
    );
}
