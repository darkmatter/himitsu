{
  description = "Himitsu — age-based secrets management with transport-agnostic sharing";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    treefmt.url = "github:numtide/treefmt-nix";
    flake-parts.url = "github:hercules-ci/flake-parts";
    flake-parts.inputs.nixpkgs-lib.follows = "nixpkgs";
  };

  outputs =
    inputs@{
      self,
      nixpkgs,
      flake-utils,
      treefmt,
      flake-parts,
    }:
    flake-parts.lib.mkFlake { inherit inputs; } (
      {
        config,
        ...
      }:
      {
        systems = flake-utils.lib.defaultSystems;
        imports = [
          treefmt.flakeModule
        ];
        perSystem =
          {
            self',
            pkgs,
            system,
            ...
          }:
          {
            # _module.args.pkgs = import inputs.nixpkgs { inherit system; };

            treefmt = {
              programs.rustfmt.enable = true;
              settings.formatter.rustfmt.options = pkgs.lib.mkForce [
                "--config"
                "skip_children=true"
                "--edition"
                "2024"
              ];
            };

            packages.himitsu = pkgs.rustPlatform.buildRustPackage {
              pname = "himitsu";
              version = "0.1.0";
              src = ./.;
              cargoLock.lockFile = ./Cargo.lock;
              nativeBuildInputs = [
                pkgs.git
                pkgs.protobuf
              ];

              meta = with pkgs.lib; {
                description = "Age-based secrets management with transport-agnostic sharing";
                license = licenses.mit;
                platforms = platforms.unix;
              };
            };
            packages.default = self'.packages.himitsu;
            # ── Helper: emit the raw age secret key from the local keyring ─
            packages.age-key-cmd = pkgs.writeShellScriptBin "himitsu-age-key-cmd" ''
              HIMITSU_HOME="''${HIMITSU_HOME:-$HOME/.himitsu}"
              KEY_FILE="$HIMITSU_HOME/keys/age.txt"

              if [ -f "$KEY_FILE" ]; then
                grep -v '^#' "$KEY_FILE"
              else
                echo "No age key found at $KEY_FILE" >&2
                exit 1
              fi
            '';

            apps.default = {
              type = "app";
              program = "${self'.packages.himitsu}/bin/himitsu";
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
                protobuf
              ];
              shellHook = ''
                alias himitsu="$(git rev-parse --show-toplevel)/target/debug/himitsu"
              '';
            };

            devShells.coverage = pkgs.mkShell {
              inputsFrom = [
                self'.devShells.default
              ];
            };

            checks = {
              himitsu = self'.packages.himitsu;
              himitsu-smoke = (
                pkgs.runCommand "himitsu-smoke" { buildInputs = [ self'.packages.himitsu ]; } ''
                  himitsu --help
                  touch $out
                ''
              );
            };
          };

        # Per-system lib for downstream flakes: himitsu.lib.${system}.mkDevShell, etc.
        # See nix/lib/default.nix and docs/nix-integration.md for the full API.
        flake.lib = nixpkgs.lib.genAttrs config.systems (
          system:
          let
            s = config.allSystems.${system};
          in
          import ./nix/lib {
            pkgs = s.pkgs;
            himitsu = s.packages.himitsu;
            age-key-cmd = s.packages.age-key-cmd;
          }
        );
      }
    )

    // {
      herculesCI = {
        onPush = { };
      };
    };
}
