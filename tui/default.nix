{ bun2nix, ... }:
bun2nix.mkDerivation {
  pname = "himitsu-tui";
  version = "1.0.0";

  src = ./.;

  bunNix = ./bun.nix;

  module = "src/index.ts";
}
