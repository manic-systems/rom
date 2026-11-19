# Changelog

## Unreleased

### Added

- `LegendStyle::Nom` (`--legend nom`) and `SummaryStyle::Nom` (`--summary nom`)
  for per-host build, download, upload, and byte-progress tables in the style of
  `nom`.
- Per-host sorting (`--host-sort none|download-size|download-count|builds`) and
  remote host capping (`--max-hosts N`, aggregating overflow hosts into
  `(other)`).
- Opt-in curl download progress parsing from builder logs (`--curl-progress
  true|false`), surfacing fetch hostnames and transferred bytes in tree rows and
  per-host tables without filtering build logs.
- Independent display toggles for progress bars (`--progress-bars true|false`),
  transfer percentages (`--show-percentages true|false`), and always-decimal
  byte formatting (`--decimal-bytes true|false`).
- `[theme]` color overrides in `$XDG_CONFIG_HOME/rom/config.toml` for all
  semantic `Theme` slots (ANSI names, 256-color indices, and `#RRGGBB` hex
  colors), and `rom::Color` re-export for library consumers.
- Child `FileTransfer` progress fallback when the parent `CopyPath`/`Substitute`
  activity does not report direct byte progress.

## 0.6.0

### Changed

- The live graph redraws inside tmux and GNU Screen instead of freezing after its first frame (#110).
- `terminal::admission` and `Admission` are replaced by `terminal::is_live`.

## 0.3.3

### Added

- rom now wraps any other nix subcommand, like `rom eval` or `rom run`.
- `LiveTerminal::flush_logs` writes pending log lines without drawing a frame.

### Fixed

- Builder log lines are written between throttled frames instead of waiting for the next one.

## 0.3.2

### Changed

- Producer lookups go through indexes by consumer and name, so a build's start and stop no longer scan every producer.
- Frames are spaced at ten times the last frame's cost, between 50 ms and 1 s.
- Shared derivation names are counted as derivations are added, and state maps hash with foldhash.
- `State::derivations` and `State::store_paths` return maps keyed with foldhash.
- pound is 0.6.1, so `--platform` parses through `FromStr` with no parser named.

### Fixed

- The `nix build` rom wraps is killed when rom exits.
- Non-Linux builds no longer warn about an unused `env` import.

## 0.3.1

### Added

- Builds waiting on a lock or a free machine show as waiting rows, with the reason muted.
- Producers an earlier run built are remembered, so a resumed run still nests its units.

### Changed

- Nix lookups spawn without forking, run at most two at a time in batches, and are killed past a timeout or when rom exits, through `setpriv` on Linux.
- Realisation checks wait until a build settles and skip derivations Nix would spin on.
- Producers fold into their package row and drop the `.drv` suffix.
- Units sit under their consumers, placed lazily as each frame needs them.
- Content-addressed derivations that only the dry run lists stay hidden until something shows they will build.
- The output check after a failed build gives up after 10 seconds.
- `Engine::poll_lookups` takes the current time.
- State, lookup and error types are `#[non_exhaustive]`.

### Fixed

- A running unit no longer shows its count once all its inputs are done.
- Rows for running units are budgeted before the units holding them.
- A unit counts as done once the derivation its scan wrote is.
- A producer whose lookup Nix refuses to start counts as unbuilt.

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
