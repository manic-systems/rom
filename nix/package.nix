{
  lib,
  rustPlatform,
  stdenv,
  buildPackages,
  installShellFiles,
}: let
  cargoTOML = (lib.importTOML ../Cargo.toml).workspace.package;
in
  rustPlatform.buildRustPackage {
    pname = "rom";
    version = cargoTOML.version;

    src = let
      fs = lib.fileset;
      s = ../.;
    in
      fs.toSource {
        root = s;
        fileset = fs.unions [
          (s + /crates)
          (s + /rom)
          (s + /scripts)
          (s + /Cargo.lock)
          (s + /Cargo.toml)
        ];
      };

    cargoLock.lockFile = ../Cargo.lock;
    enableParallelBuildingByDefault = true;

    nativeBuildInputs = [installShellFiles];

    postInstall = lib.optionalString (stdenv.hostPlatform.emulatorAvailable buildPackages) (
      let
        rom = "${stdenv.hostPlatform.emulator buildPackages} $out/bin/rom";
      in ''
        installShellCompletion --cmd rom \
          --bash    <(COMPLETE=bash ${rom}) \
          --zsh     <(COMPLETE=zsh  ${rom}) \
          --fish    <(COMPLETE=fish ${rom}) \
          --nushell <(COMPLETE=nu   ${rom})
      ''
    );

    meta = {
      description = "Pretty build graphs for your pretty Nix builds";
      homepage = "https://github.com/feel-co/rom";
      maintainers = with lib.maintainers; [NotAShelf];
      license = lib.licenses.eupl12;
    };
  }
