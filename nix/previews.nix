{
  mkShellNoCC,
  asciinema,
  asciinema-agg,
  nushell,
  nerd-fonts,
}:
mkShellNoCC {
  name = "rom-previews";

  packages = [
    asciinema
    asciinema-agg
    nushell
  ];

  env.ROM_PREVIEW_FONTS = "${nerd-fonts.jetbrains-mono}/share/fonts";
}
