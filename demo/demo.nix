{
  pkgs,
  lib,
  config,
  options,
  ...
}:
let
  cfg = config.vhs;
in
{
  options.vhs = {
    enable = lib.mkEnableOption "VHS terminal recorder";
    typingSpeedMs = lib.mkOption {
      type = lib.types.int;
      default = 45;
    };
    pauseBetweenSectionsMs = lib.mkOption {
      type = lib.types.int;
      default = 2500;
    };
    pauseBetweenCommandsMs = lib.mkOption {
      type = lib.types.int;
      default = 800;
    };
    newlinesBetweenCommands = lib.mkOption {
      type = lib.types.int;
      default = 1;
    };
    newlinesBetweenSections = lib.mkOption {
      type = lib.types.int;
      default = 2;
    };
    tape = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
    };
  };

  config.vhs = lib.mkIf cfg.enable {
    fileContents =
      let
        mkLineBreak = n: builtins.genList (i: "Enter") n;
        brAfterCommand = mkLineBreak cfg.newlinesBetweenCommands;
        brSectionBreak = mkLineBreak cfg.newlinesBetweenSections;
        handlers = {
          command =
            cmd:
            (lib.concatLists [
              "Type \"${cmd}\""
              "Enter"
            ] brAfterCommand);
          wait = ms: [ "Sleep ${toString ms}" ];
          hiddenCommand = cmd: [
            "Hide"
            "Type \"${cmd}\""
            "Enter"
            "Show"
          ];
        };
        tapelines = lib.mapAttrsToList (name: cmd: handlers.${name} cmd) cfg.tape;
        getLineType =
          line:
          if builtins.match "\$" line != null then
            "command"
          else if builtins.match "Sleep \\d+" line != null then
            "wait"
          else if builtins.match "Hide" line != null then
            "hiddenCommand"
          else
            null;
        tapeContents = lib.concatMapStringsSep "\n" (line: line) tapelines;
      in
      tapeContents;
  };
}
