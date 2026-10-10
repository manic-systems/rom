//! Minimalistic parsers for Nix's `ATerm` `.drv` files and
//! `internal-json` log format.

pub mod aterm;
pub mod internal;
mod state;

pub use aterm::{
  ParsedDerivation,
  extract_pname,
  extract_version,
  parse_drv_file,
};
pub use internal::{
  Platform,
  json::{
    Actions,
    Activities,
    DecodedAction,
    Id,
    ResultType,
    UnsupportedRecord,
    Verbosity,
    decode_action,
  },
};
pub use state::{Host, OutputName, ProgressState};
