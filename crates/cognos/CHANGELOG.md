# Changelog

## 2.0.0

### Added

- `DrvWithVersion` derivations, and `ParsedDerivation::dynamic_drvs` for inputs whose outputs are themselves derivations.
- `decode_action`, which tells an unsupported internal-JSON record (`DecodedAction::Unsupported`) apart from a malformed one.
- `FetchToStore` activities and results.

### Changed

- `.drv` files are parsed from bytes. `parse_drv_content` takes `&[u8]`, and environment values are a `BTreeMap<String, Vec<u8>>`, since they need not be UTF-8.
- Parsing fails with `aterm::Error` and `ParseError`, which carry the byte offset, instead of a `String`. A duplicate environment variable is an error.
- `Platform::from_str` returns `UnknownPlatform` and only accepts `nix` and `lix`.
