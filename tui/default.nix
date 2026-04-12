{ bun2nix, ... }:
bun2nix.mkDerivation {
  pname = "himitsu-tui";
  version = "1.0.0";

  src = ./.;

  bunDeps = bun2nix.fetchBunDeps {
    bunNix = ./bun.nix;
  };

  module = "src/index.ts";

  # @opentui/core uses top-level await, which is incompatible with --bytecode (CJS mode)
  bunCompileToBytecode = false;
}
