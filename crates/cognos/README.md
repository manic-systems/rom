# Cognos

[rom]: https://github.com/manic-systems/rom

Minimalistic parser for Nix's ATerm `.drv` and `internal-json` log formats.

Cognos is the parsing subcrate that powers [rom]. It provides
zero-dependency-on-Nix parsing of derivation files and the structured log lines
Nix (and Lix) emit with `--log-format internal-json`.

## Features

- Parse `.drv` files in ATerm format into a structured `ParsedDerivation`.
- Parse `@nix`-prefixed `internal-json` log lines into typed `Actions`.
- Detect the active Nix-family evaluator (Nix vs. Lix).
- Lightweight: only depends on `serde`, `serde_json`, and `serde_repr`.

## Usage

### Parsing a derivation file

```rust
use cognos::{parse_drv_file, extract_pname, extract_version};

let drv = parse_drv_file("/nix/store/...-hello-2.12.1.drv")?;

println!("pname:  {:?}", extract_pname(&drv.env));
println!("version: {:?}", extract_version(&drv.env));

for (path, _) in &drv.input_drvs {
    println!("depends on: {path}");
}
```

`parse_drv_content` does the same for an in-memory string, which is useful when
you already have the ATerm text (e.g. from `nix derivation show`):

```rust
use cognos::parse_drv_content;

let drv = parse_drv_content(content)?;
```

### Parsing build logs

Nix and Lix emit one JSON object per line prefixed with `@nix` when run with
`--log-format internal-json`. `parse_line` turns each line into a typed
`Actions` value:

```rust
use cognos::internal::json::{parse_line, Actions, Verbosity};

for line in log_lines {
    if let Some(Actions::Message { level, msg, .. }) = parse_line(&line) {
        if level <= Verbosity::Error {
            eprintln!("{msg}");
        }
    }
}
```

The `Actions` enum covers `Start`, `Stop`, `Message`, and `Result` actions.
Lix-specific source-location fields (`raw_msg`, `file`, `line`, `column`) are
parsed transparently via serde defaults, so the same code handles both Nix and
Lix output.

### Detecting the evaluator

```rust
use cognos::Platform;

let platform = Platform::detect();
println!("binary: {}", platform.binary());
```

`Platform` implements `FromStr`, so you can parse user input directly:

```rust
use std::str::FromStr;

let platform = cognos::Platform::from_str("lix").unwrap();
```

## Attribution

The ATerm and internal-json log parser was inspired by
[nous](https://git.atagen.co/atagen/nous), with consolidation and a cleaner
separation of concerns.

## License

Licensed under the European Union Public Licence v. 1.2
([EUPL-1.2](https://joinup.ec.europa.eu/collection/eupl/eupl-text-eupl-12)).
