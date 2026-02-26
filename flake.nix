{
  description = "Himitsu - SOPS-based secrets framework";

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
        ];

        himitsu = pkgs.stdenv.mkDerivation {
          pname = "himitsu";
          version = "0.1.0";
          src = ./src;

          nativeBuildInputs = [ pkgs.makeWrapper ];

          dontBuild = true;

          installPhase = ''
            runHook preInstall

            mkdir -p $out/bin $out/lib/himitsu
            cp lib/*.sh $out/lib/himitsu/
            cp bin/himitsu $out/bin/himitsu
            chmod +x $out/bin/himitsu

            wrapProgram $out/bin/himitsu \
              --prefix PATH : ${pkgs.lib.makeBinPath runtimeDeps} \
              --set HIMITSU_LIB "$out/lib/himitsu"

            runHook postInstall
          '';

          meta = with pkgs.lib; {
            description = "SOPS-based secrets management with group recipient control";
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
          packages = runtimeDeps ++ (with pkgs; [
            bats
            shellcheck
          ]);

          shellHook = ''
            export HIMITSU_LIB="$(pwd)/src/lib"
          '';
        };
      }
    );
}
