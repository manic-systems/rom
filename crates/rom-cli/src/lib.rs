//! ROM CLI - command-line interface and nix process wrappers
mod cli;
mod log_store;

pub use cli::{
  Cli,
  Commands,
  parse_args_with_separator,
  replace_command_with_exit,
  run,
};
