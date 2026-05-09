{
  description = "Himitsu — age-based secrets management with transport-agnostic sharing";

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
        pkgs = nixpkgs.legacyPackages.${system};

        # ── Core binary ──────────────────────────────────────────────
        himitsu = pkgs.rustPlatform.buildRustPackage {
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

        # ── Helper: emit the raw age secret key from the local keyring ─
        age-key-cmd = pkgs.writeShellScriptBin "himitsu-age-key-cmd" ''
          HIMITSU_HOME="''${HIMITSU_HOME:-$HOME/.himitsu}"
          KEY_FILE="$HIMITSU_HOME/keys/age.txt"

          if [ -f "$KEY_FILE" ]; then
            grep -v '^#' "$KEY_FILE"
          else
            echo "No age key found at $KEY_FILE" >&2
            exit 1
          fi
        '';

        # ── Library: mkDevShell, packSecrets, wrapAge, … ─────────────
        himitsuLib = import ./nix/lib {
          inherit pkgs himitsu age-key-cmd;
        };
      in
      {
        # ── packages ───────────────────────────────────────────────────
        packages = {
          default = himitsu;
          himitsu = himitsu;
          inherit age-key-cmd;
        };

        # ── lib ────────────────────────────────────────────────────────
        #
        # Consumer usage (in a downstream flake):
        #
        #   # Simplest — point at a store dir, get a wired-up devShell:
        #   devShells.default = himitsu.lib.${system}.mkDevShell {
        #     devShell = pkgs.mkShell { packages = [ nodejs ]; };
        #     store    = ./.himitsu;
        #     env      = "dev";
        #   };
        #
        #   # With pre-packed secrets:
        #   let secrets = himitsu.lib.${system}.packSecrets ./.himitsu/vars/dev;
        #   in himitsu.lib.${system}.mkDevShell {
        #     devShell = myShell;
        #     secrets  = secrets;
        #   };
        #
        #   # Credential server OCI image:
        #   packages.secrets-server = himitsu.lib.${system}.mkCredentialServerImage {
        #     store = ./.himitsu;
        #     env   = "prod";
        #     port  = 9292;
        #   };
        #
        # Full API surface:
        #
        #   packSecrets              — collect .age files into a derivation
        #   mkDevShell               — wrap any devShell with secret injection
        #   wrapAge                  — age binary that auto-injects identity
        #   wrapSops                 — sops binary that auto-discovers key
        #   mkCredentialServer       — standalone HTTP credential server script
        #   mkEntrypoint             — container ENTRYPOINT that decrypts + exec's CMD
        #   mkSecretsLayer           — encrypted .age tar layer for OCI images / skopeo / crane
        #   mkCredentialServerImage  — minimal OCI image with the credential server
        #
        lib = himitsuLib;

        # ── apps ───────────────────────────────────────────────────────
        apps.default = {
          type = "app";
          program = "${himitsu}/bin/himitsu";
        };

        # ── devShells ──────────────────────────────────────────────────
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
            self.devShells.${system}.default
          ];
        };

        # ── checks ─────────────────────────────────────────────────────
        checks = {
          himitsu = himitsu; # build runs cargo test
          himitsu-smoke =
            pkgs.runCommand "himitsu-smoke"
              {
                buildInputs = [ himitsu ];
              }
              ''
                himitsu --help
                touch $out
              '';
        };
      }
    )
    // {
      herculesCI = {
        onPush = { };
      };
    };
}
