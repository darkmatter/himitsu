{
  description = "Himitsu - age-based secrets management";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        himitsu = pkgs.rustPlatform.buildRustPackage {
          pname = "himitsu";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.git ];

          meta = with pkgs.lib; {
            description = "Age-based secrets management with transport-agnostic sharing";
            license = licenses.mit;
            platforms = platforms.unix;
          };
        };
      in
      {
        packages = {
          default = himitsu;
          himitsu = himitsu;
        };

        apps.default = {
          type = "app";
          program = "${himitsu}/bin/himitsu";
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            git
            gh
            age
          ];
        };

        checks = {
          himitsu = himitsu;  # build runs cargo test
          himitsu-smoke = pkgs.runCommand "himitsu-smoke" {
            buildInputs = [ himitsu ];
          } ''
            himitsu --help
            touch $out
          '';
        };
      }
    );
}
