{
  description = "Panel for the COSMIC desktop environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, utils }:
    utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };

        runtimeDependencies = with pkgs; [
          wayland
          libGL
        ];

      in {
        devShells.default = with pkgs; mkShell rec {
          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs = with pkgs; [
            bashInteractive
            libxkbcommon
            stdenv.cc.cc.lib
          ];

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeDependencies;
        };
      }
    );
}
