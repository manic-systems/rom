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
pub mod update;

pub use error::{Result, RomError};
pub use monitor::{
  DerivationResolver,
  Engine,
  FilesystemResolver,
  Monitor,
  Output,
  Processed,
  StreamEngine,
  create_monitor,
  monitor_stream,
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

/// Run the CLI application with the provided arguments.
///
/// This is the main entry point for the CLI application.
#[cfg(feature = "cli")]
pub fn run() -> eyre::Result<()> {
  cli::run()
}
