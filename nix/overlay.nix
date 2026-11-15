final: _prev: {
  rom = final.lib.makeScope final.newScope (self: {
    rom = self.callPackage ./package.nix { };

    dev-shell = self.callPackage ./shell.nix { };

    previews-shell = self.callPackage ./previews.nix { };

    default = self.rom;
  });
}
