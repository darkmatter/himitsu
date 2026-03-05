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

        runtimeDeps = with pkgs; [
          sops
          age
          jq
          yq-go
          gnugrep
          gnused
          coreutils
          findutils
          gh
          git
        ];

        # Legacy shell implementation
        himitsu-shell = pkgs.stdenv.mkDerivation {
          pname = "himitsu-shell";
          version = "0.1.0";
          src = ./src;

          nativeBuildInputs = [ pkgs.makeWrapper ];

          dontBuild = true;

          installPhase = ''
            runHook preInstall

            mkdir -p $out/bin $out/lib/himitsu
            cp lib/*.sh $out/lib/himitsu/
            cp bin/himitsu $out/bin/himitsu-shell
            chmod +x $out/bin/himitsu-shell

            wrapProgram $out/bin/himitsu-shell \
              --prefix PATH : ${pkgs.lib.makeBinPath runtimeDeps} \
              --set HIMITSU_LIB "$out/lib/himitsu"

            runHook postInstall
          '';

          meta = with pkgs.lib; {
            description = "Himitsu shell implementation (legacy)";
            license = licenses.mit;
            platforms = platforms.unix;
          };
        };

        # Rust implementation
        himitsu = pkgs.rustPlatform.buildRustPackage {
          pname = "himitsu";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

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
          himitsu-shell = himitsu-shell;
        };

        apps.default = {
          type = "app";
          program = "${himitsu}/bin/himitsu";
        };

        devShells.default = pkgs.mkShell {
          packages = runtimeDeps ++ (with pkgs; [
            bats
            shellcheck
            cargo
            rustc
            clippy
            rustfmt
          ]);

          shellHook = ''
            export HIMITSU_LIB="$(pwd)/src/lib"
          '';
        };
      }
    );
}
