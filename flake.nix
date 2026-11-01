{
  inputs.nixpkgs.url = "https://channels.nixos.org/nixos-unstable/nixexprs.tar.zst";

  outputs =
    {
      self,
      nixpkgs,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forEachSystem = nixpkgs.lib.genAttrs systems;
      pkgsForEach = nixpkgs.legacyPackages;
    in
    {
      packages = forEachSystem (system: {
        rom = pkgsForEach.${system}.callPackage ./nix/package.nix { };
        default = self.packages.${system}.rom;
      });

      devShells = forEachSystem (system: {
        default = pkgsForEach.${system}.callPackage ./nix/shell.nix { };
        previews = pkgsForEach.${system}.callPackage ./nix/previews.nix { };
      });

      hydraJobs = self.packages;
    };
}
