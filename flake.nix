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
      pkgsForEach = forEachSystem (system: import nixpkgs {
        inherit system;
        overlays = [ self.overlays.default ];
      });
    in
    {
      overlays.default = import ./nix/overlay.nix;

      packages = forEachSystem (system: {
        inherit (pkgsForEach.${system}.rom)
          rom
          default
          ;
      });

      devShells = forEachSystem (system: {
        default = pkgsForEach.${system}.rom.dev-shell;
        previews = pkgsForEach.${system}.rom.previews-shell;
      });

      hydraJobs = self.packages;
    };
}
