//! ROM - a Nix and Lix build output monitor.
#![allow(clippy::module_name_repetitions)]

pub mod cache;
#[cfg(feature = "cli")] pub mod cli;
pub mod display;
pub mod error;
mod event;
pub mod icons;
pub mod monitor;
pub mod state;
pub mod terminal;
pub mod types;
mod update;

pub use error::{Result, RomError};
pub use monitor::{
  DerivationResolver,
  Engine,
  FilesystemResolver,
  Monitor,
  Output,
  Processed,
  StreamEngine,
};
pub use types::{
  Config,
  DisplayFormat,
  EngineConfig,
  IconMode,
  InputMode,
  LegendStyle,
  LogLine,
  LogPrefixStyle,
  RenderConfig,
  SummaryStyle,
  Theme,
};

/// Runs the CLI with the process's command-line arguments.
///
/// # Errors
///
/// Returns an error if an argument is not valid UTF-8, the Nix process cannot
/// be run, or the build fails or ends with unfinished work.
#[cfg(feature = "cli")]
pub fn run() -> misstep::Result<()> {
  cli::run()
}
