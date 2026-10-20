# Changelog

## 0.3.0

### Added

- Dynamic derivations. A drv a producer writes is linked to its consumers through Nix's build trace, and its units fold under one package row with a `units 768/825, 32 running` summary.
- Content-addressed derivations Nix finds already realised are marked done without a build.
- Remote builds name their machine after the timer, as `⏱ 3s @rauk`, and a package row counts its running units per machine.
- Sources being copied into the store show as rows of their own.

### Changed

- `rom-core` and `rom-cli` are folded into this crate, built around a new render tree, and are no longer published.
- Concurrent runs merge their build history under a file lock instead of overwriting it.
- The graph is a light tree over a heavy status box, coloured by state alone, and the status table fits narrow terminals.
- The live graph stays anchored and on screen while it grows.
- The wrapper forwards installables and interrupts to Nix and exits with Nix's own code.
- Nix errors are classified by kind, and messages are filtered by verbosity.
- Errors are reported through `misstep`.
- `--platform` takes exactly `nix` or `lix`.
