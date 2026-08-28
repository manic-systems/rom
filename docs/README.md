# ROM

ROM turns Nix and Lix `internal-json` logs into a connected, real-time build
graph. It is both an installable command and a synchronous Rust library.

ROM is under active development. Its Rust API and presentation are explicitly
unstable and carry no compatibility guarantee yet.

## Commands and streams

Wrap the evaluator directly:

```console
rom build nixpkgs#hello
rom shell nixpkgs#python3
rom develop .#default
```

Arguments following `--` are passed to Nix or Lix:

```console
rom build nixpkgs#hello -- --rebuild
```

ROM also accepts an existing stream. By default it decodes lines beginning
with `@nix ` and passes every other byte through exactly once:

```console
nix build nixpkgs#hello -v --log-format internal-json 2>&1 | rom
```

Use `--json` only for an unprefixed stream containing one internal-JSON object
per record. A malformed record that claims to be structured input is an error.

ROM sends its presentation, decoded logs, passthrough, and diagnostics to
stderr. A wrapped child's stdout is relayed concurrently and byte-for-byte to
stdout, so store paths remain safe to pipe into another program.

On an interactive stderr, decoded log records retain their ANSI colors and
attributes and ROM colors their activity prefixes. On a redirected stderr,
decoded records are stripped of ANSI. Non-protocol passthrough is always
byte-exact, so producer-supplied escape bytes in passthrough remain untouched.

## Presentations

The existing tree, plain, and dashboard formats remain available, along with
compact/table/verbose legends, concise/table/full summaries, log-prefix modes,
timers, automatic Nerd Font detection, and the `NERD_FONTS=0` or
`NERD_FONTS=1` override.

Downloads and uploads are first-class activities. Known-size transfers use the
block bar `█▛▌▖  `, with semantic colors supplied by the active `Theme`.
Unknown-size transfers show transferred bytes and a spinner rather than a fake
percentage.

Like nix-output-monitor, ROM uses synchronized updates on every direct
interactive terminal without a capability round trip. Pending logs and the
complete connected graph are composed into one write; the previous graph is
cleared line-by-line in that same transaction. ROM deliberately avoids a
protected scrolling region. tmux and GNU Screen remain on the no-cursor path
until their passthrough behavior is validated: one initial connected graph,
then logs and the final summary. Redirected output remains logs plus the final
summary. As in nix-output-monitor, producer controls are preserved without
classification and are emitted with the pending logs inside the next atomic
graph commit. ROM uses the normal screen and preserves scrollback.

The live tree reserves its bottom rows for the selected legend and limits the
dependency graph above it to two-thirds of the usable terminal height. Direct
live rendering requires at least 20 columns by 8 rows; smaller terminals use
the append-only fallback rather than dropping legend rows or drawing a
disconnected tree.

## Library

`Engine` is the deterministic reducer. It owns no terminal, threads, global
clock, or async runtime:

```rust
use rom::{monitor::Engine, EngineConfig};

let mut engine = Engine::new(EngineConfig::default());
let update = engine.process_record_at(
    br#"@nix {"action":"msg","level":3,"msg":"hello"}"#,
    0.0,
)?;
```

Library-only consumers can disable ROM's default `cli` feature to omit Clap,
the tracing subscriber, and Unix process-signal dependencies.

`StreamEngine` adds incremental framing without accumulating arbitrarily long
passthrough lines. `Monitor` is the append-only `BufRead`/`Write` convenience
adapter. Build-history storage and `.drv` resolution are optional injected
services; the CLI supplies filesystem-backed implementations.

## Reproducible visual fixtures

Each fixture keeps its timed input and every expected checkpoint together:

```text
rom/tests/fixtures/<fixture-name>/
├── events.jsonl
└── expected/
    ├── plain-79x23/
    │   ├── frames.txt
    │   ├── stream.txt
    │   └── <named-checkpoint>.txt
    └── ansi-79x23/
        ├── frames.txt
        ├── stream.txt
        └── <named-checkpoint>.txt
```

Rust tests replay timestamps with a virtual clock. Every input write,
checkpoint, and EOF is captured in `frames.txt`; `stream.txt` captures decoded
logs. The plain variant asserts that no escape byte survives, while the ANSI
variant stores exact SGR sequences as visible `\\e` escapes. A PTY integration
test additionally runs the real CLI and proves that the synchronized live path
commits styled and over-width logs together with a complete graph, while the
no-cursor multiplexer fallback still shows the first graph:

```console
cargo test --test fixtures fixture_download_progress
cargo test --test fixtures fixture_nixpkgs_hello
cargo test --test fixtures fixture_nested_builds
cargo test --test pty_live
```

Expectations are changed only through an explicit Miri-style blessing pass:

```console
ROM_BLESS_EXPECTED=1 cargo test fixture_download_progress
git diff -- rom/tests/fixtures
```

The same event file can be watched through the real binary:

```console
cargo build -p rom
uv run scripts/replay-fixture.py download-progress
uv run scripts/replay-fixture.py download-progress --speed 4
uv run scripts/replay-fixture.py download-progress --step
uv run scripts/replay-fixture.py download-progress -- --format dashboard
uv run scripts/replay-fixture.py nixpkgs-hello --speed 4
uv run scripts/replay-fixture.py nested-builds --step
```

Arguments after `--` are passed to ROM. The replay helper never builds ROM
implicitly.

## Attribution

ROM is inspired by
[nix-output-monitor](https://github.com/maralorn/nix-output-monitor). Cognos's
ATerm and internal-JSON parser was inspired by
[nous](https://git.atagen.co/atagen/nous).

## License

ROM and Cognos are licensed under the European Union Public Licence v. 1.2
([EUPL-1.2](https://joinup.ec.europa.eu/collection/eupl/eupl-text-eupl-12)).
See [`LICENSE`](../LICENSE).
